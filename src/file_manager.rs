use crate::{manifest::Manifest, memmap::MemMap};
use anyhow::{anyhow, Context};
use rand::seq::IteratorRandom;
use sha1::{Digest, Sha1};
use std::{
    cmp::Eq,
    env,
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::{ErrorKind, SeekFrom},
    ops::Deref,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};
use tracing::{debug, error, warn};

#[derive(Default, Debug, Clone)]
struct Chunk {
    index: usize,
    data: Vec<u8>,
}

impl Chunk {
    fn new_dummy_chunks(chunk_count: usize) -> Vec<Chunk> {
        let mut chunks = vec![Chunk::default(); chunk_count];

        chunks
            .iter_mut()
            .enumerate()
            .for_each(|(i, chunk): (usize, &mut Chunk)| {
                chunk.index = i;
            });

        chunks
    }
}

#[derive(Debug)]
pub struct FileManager<RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin> {
    manifest: Manifest,
    file: Mutex<RWS>,
    dummy_chunks: Mutex<Vec<Chunk>>,
}

impl FileManager<MemMap> {
    #[tracing::instrument(err)]
    pub async fn new(manifest: Manifest) -> anyhow::Result<Self> {
        let mut filepath = env::current_dir().expect("current dir should be present");
        filepath.push("download/");

        fs::create_dir_all(filepath.as_path())
            .with_context(|| "failed to a create a directory for downloaded files")?;

        let filename = manifest.filename();
        filepath.push(filename);

        let file = tokio::task::spawn_blocking(move || {
            OpenOptions::new().write(true).read(true).create(true).open(filepath.as_path()).context(format!("failed to create file {:?}", filepath.as_path()))
        })
        .await??;

        let mem_mapped_file = Mutex::new(MemMap::new(file)?);

        let sharps_size = manifest.sharps().len();
        let dummy_chunks = Chunk::new_dummy_chunks(sharps_size);

        Ok(Self {
            manifest,
            file: mem_mapped_file,
            dummy_chunks: Mutex::new(dummy_chunks),
        })
    }
}

impl<RWS> FileManager<RWS>
where
    RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin,
{
    #[tracing::instrument(skip(self), fields(manifest, chunks, result), ret)]
    pub async fn absent_chunk_index(&self) -> Option<usize> {
        let dummy_chunks = self.dummy_chunks.lock().await;
        dummy_chunks
            .iter()
            .filter(|dummy_chunk| dummy_chunk.data.is_empty())
            .choose(&mut rand::thread_rng())
            .map(|dummy_chunk| dummy_chunk.index)
    }

    #[tracing::instrument(skip(self), fields(manifest, chunks), err)]
    pub async fn fill_chunk(&self, possibly_suitable_chunk: Vec<u8>) -> anyhow::Result<()> {
        let mut sha1 = Sha1::new();
        sha1.update(&possibly_suitable_chunk);
        let possibly_suitable_chunk_hash = sha1.finalize();

        let dummy_chunk_index = self
            .manifest
            .sharps()
            .iter()
            .enumerate()
            .find(|(_, sharp)| &possibly_suitable_chunk_hash.deref() == sharp)
            .map(|(i, _)| i)
            .ok_or_else(|| {
                anyhow!(format!(
                    "{possibly_suitable_chunk:#?} is not related to {}",
                    self.manifest.filename()
                ))
            })?;

        {
            let mut dummy_chunks = self.dummy_chunks.lock().await;

            let dummy_chunk = dummy_chunks.iter_mut().find(|chunk| chunk.index == dummy_chunk_index);
            if dummy_chunk.is_none() {
                error!("didn't find a correspond dummy chunk to {dummy_chunk_index} index, but some sharp is equal to this chunk hash");
            }

            let dummy_chunk = dummy_chunk.unwrap();
            dummy_chunk.data = possibly_suitable_chunk;
            debug!("filled chunk at index {}", dummy_chunk.index);
        }

        self.loadout_filled_chunks().await?;

        Ok(())
    }

    #[tracing::instrument(skip(self), fields(manifest, chunks), err)]
    async fn loadout_filled_chunks(&self) -> anyhow::Result<()> {
        let mut dummy_chunks = self.dummy_chunks.lock().await;

        if dummy_chunks
            .iter()
            .take_while(|dummy_chunk| !dummy_chunk.data.is_empty())
            .next()
            .is_none()
        {
            debug!("there are not a sequence of filled chunks, cannot loadout chunks");
            return Ok(());
        }

        let filled_chunks_count = dummy_chunks
            .iter()
            .take_while(|dummy_chunk| !dummy_chunk.data.is_empty())
            .count();

        debug_assert!(
            filled_chunks_count == 0,
            "filled chunks count is zero, but sum of filled chunks' data size is greater than 20MB"
        );

        let (filled_chunks, _) = dummy_chunks.split_at(filled_chunks_count);
        let mut to_remove_elements_count = 0;
        let mut write_result = Ok(());

        debug!(
            "writing to {} file {} chunks of data",
            self.manifest.filename(),
            filled_chunks.len()
        );

        {
            let mut file = self.file.lock().await;

            for filled_chunk in filled_chunks {
                to_remove_elements_count += 1;
                write_result = file.write_all(filled_chunk.data.as_ref()).await;

                if write_result.is_err() {
                    break;
                }

                debug!(
                    "wrote {} bytes to {}",
                    filled_chunk.data.len(),
                    self.manifest.filename()
                );
            }
        }

        debug!(
            "wrote {to_remove_elements_count} chunks to {}",
            self.manifest.filename()
        );

        dummy_chunks.drain(0..to_remove_elements_count);
        write_result.map_err(Into::into)
    }

    #[tracing::instrument(skip(self), fields(manifest, chunks), err)]
    pub async fn filled_chunk(&self, index: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let mut file = self.file.lock().await;

        let piece_size = self.manifest.piece_size();
        let pos = match file.seek(SeekFrom::Start((piece_size * index) as u64)).await {
            Ok(pos) => Some(pos),
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => None,
            Err(err) => return Err(err.into()),
        };

        let chunk = if pos.is_some() {
            let mut buf = Vec::with_capacity(piece_size);
            let were_read = file.read_exact(&mut buf).await?;

            if were_read != piece_size {
                warn!(
                    "read {were_read}, but a piece size is {piece_size}. It can be ok if we are at the end of a file."
                );
            }

            Some(buf)
        } else {
            // needed chunk is in the buffed chunks (can happen if where was not a chance to write the chunk in a file)
            // or absent
            self.dummy_chunks.lock().await.iter().find_map(|chunk| {
                if chunk.index == index && !chunk.data.is_empty() {
                    Some(chunk.data.clone())
                } else {
                    None
                }
            })
        };

        file.rewind().await.with_context(|| {
            format!(
                "we might find needed chunk under {index} index({}), but the file rewind failed",
                chunk.is_some()
            )
        })?;

        Ok(chunk)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

impl<RWS> PartialEq<Self> for FileManager<RWS>
where
    RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin,
{
    fn eq(&self, other: &Self) -> bool {
        self.manifest.eq(&other.manifest)
    }
}

impl<RWS> Eq for FileManager<RWS> where RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin {}

impl<RWS> Hash for FileManager<RWS>
where
    RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin,
{
    fn hash<H: Hasher>(&self, mut state: &mut H) {
        self.manifest.hash(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestInner;

    fn file_manager() -> FileManager<Vec<u8>> {
        FileManager {
            manifest: Manifest {
                inner: ManifestInner {
                    filename: "myfile".to_string(),
                    file_size: 10,
                    piece_size: 1,
                    sharps: vec![
                        vec![1, 1, 1, 1, 1],
                        vec![2, 2, 2, 2, 2],
                        vec![3, 3, 3, 3, 3],
                        vec![4, 4, 4, 4, 4],
                        vec![5, 5, 5, 5, 5],
                        vec![6, 6, 6, 6, 6],
                        vec![7, 7, 7, 7, 7],
                        vec![8, 8, 8, 8, 8],
                        vec![9, 9, 9, 9, 9],
                        vec![10, 10, 10, 10, 10],
                    ],
                },
                infohash: vec![0, 0, 0, 0, 0],
            },
            file: Mutex::new(Vec::new()),
            dummy_chunks: Mutex::new(Chunk::new_dummy_chunks(10)),
        }
    }

    #[tokio::test]
    async fn absent_chunk_should_not_return_the_same_chunk() {
        let file_manager = file_manager();

        let chunk1 = file_manager.absent_chunk_index().await.unwrap();
        let chunk2 = file_manager.absent_chunk_index().await.unwrap();

        assert_ne!(chunk1, chunk2);
    }

    #[tokio::test]
    async fn absent_chunk_never_returns_filled_chunk() {
        let file_manager = file_manager();

        let filled_chunk_sharp = {
            let mut chunks = file_manager.dummy_chunks.lock().await;
            chunks[3].data = vec![0, 0, 0, 0];

            file_manager.manifest.sharps()[3].clone()
        };

        for _ in 0..100 {
            let suggested_chunk = file_manager.absent_chunk_index().await.unwrap();
            assert_ne!(suggested_chunk, filled_chunk_sharp);
        }
    }
}
