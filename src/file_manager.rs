use crate::manifest::Manifest;
use anyhow::{anyhow, Context};
use log::{debug, error, trace};
use sha1::{Digest, Sha1};
use std::{
    cmp::Eq,
    env, fs,
    hash::{Hash, Hasher},
    ops::Deref,
};
use tokio::{fs::File, io::AsyncWriteExt, sync::Mutex};

#[derive(Default, Debug, Clone)]
struct Chunk {
    pub index: usize,
    pub data: Vec<u8>,
}

pub struct FileManager {
    manifest: Manifest,
    file: Mutex<File>,
    dummy_chunks: Mutex<Vec<Chunk>>,
}

impl FileManager {
    pub async fn new(manifest: Manifest) -> anyhow::Result<Self> {
        let mut filepath = env::current_dir().expect("current dir should be present");
        filepath.push("download/");

        fs::create_dir_all(filepath.as_path())
            .with_context(|| "failed to a create a directory for downloaded files")?;

        let filename = manifest.filename();
        filepath.push(filename);

        let file = Mutex::new(
            File::create(filepath.as_path())
                .await
                .context(format!("failed to create file {:?}", filepath.as_path()))?,
        );

        let sharps_size = manifest.sharps().len();
        let mut dummy_chunks: Vec<Chunk> = Vec::with_capacity(sharps_size);
        dummy_chunks.iter_mut().enumerate().for_each(|(i, chunk)| {
            chunk.index = i;
        });

        Ok(Self {
            manifest,
            file,
            dummy_chunks: Mutex::new(dummy_chunks),
        })
    }

    pub async fn absent_chunk(&self) -> Option<Vec<u8>> {
        let chunks_filled = self.dummy_chunks.lock().await;
        chunks_filled
            .iter()
            .find(|dummy_chunk| !dummy_chunk.data.is_empty())
            .map(|dummy_chunk| (self.manifest.sharps()[dummy_chunk.index].clone()))
    }

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
            .ok_or(anyhow!(format!(
                "{possibly_suitable_chunk:#?} is not related to {}",
                self.manifest.filename()
            )))?;

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

        self.write_filled_chunks_into_file().await?;

        Ok(())
    }

    async fn write_filled_chunks_into_file(&self) -> anyhow::Result<()> {
        let mut dummy_chunks = self.dummy_chunks.lock().await;

        if let Some(overall_filled_chunks_size) = dummy_chunks
            .iter()
            .take_while(|dummy_chunk| !dummy_chunk.data.is_empty())
            .map(|dummy_chunk| dummy_chunk.data.len())
            .reduce(|sum, chunk_size| sum + chunk_size)
        {
            if overall_filled_chunks_size < 10 * 1024 * 1024 {
                trace!("overall filled chunks size is less than 20MB");
                return Ok(());
            }
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
            "writing to {} file {} chunks with data",
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

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

impl PartialEq<Self> for FileManager {
    fn eq(&self, other: &Self) -> bool {
        self.manifest.filename().eq(other.manifest.filename())
    }
}

impl Eq for FileManager {}

impl Hash for FileManager {
    fn hash<H: Hasher>(&self, mut state: &mut H) {
        self.manifest.hash(&mut state);
    }
}
