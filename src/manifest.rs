use anyhow::{anyhow, Context};
use futures::{SinkExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    fmt::Debug,
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
};
use tokio::{fs::File, io::AsyncReadExt};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

pub type ManifestsT = Arc<Manifest>;

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone, Hash)]
pub struct ManifestInner {
    pub filename: String,
    pub file_size: usize,
    pub piece_size: usize,
    pub sharps: Vec<Vec<u8>>,
}

impl ManifestInner {
    #[tracing::instrument(fields(result))]
    fn infohash(&self) -> Vec<u8> {
        let json_view = serde_json::to_vec(&self).expect("a Manifest to JSON manifest serialization failed");

        let mut hasher = Sha1::new();
        hasher.update(&json_view);

        hasher.finalize().to_vec()
    }
}

#[derive(Debug)]
pub struct Manifest {
    pub inner: ManifestInner,
    pub infohash: Vec<u8>,
}

impl Manifest {
    #[tracing::instrument(err)]
    pub async fn from_json_file(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path).await.context(format!("Failed to open {path:?}"))?;

        let mut reader = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(file, LengthDelimitedCodec::new()),
            SymmetricalJson::<ManifestInner>::default(),
        );

        let inner = reader
            .try_next()
            .await
            .map(|value| value.expect("Manifest cannot be optional"))?; //map_err(|err| err.into())?;

        Ok(Manifest {
            infohash: inner.infohash(),
            inner,
        })
    }

    #[tracing::instrument(err)]
    pub async fn from_path(path: &Path) -> anyhow::Result<Self> {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str().map(ToString::to_string))
            .ok_or_else(|| anyhow!("failed to get {path:?} file filename"));

        let filename = filename?;

        let mut file = File::open(path).await.context(format!("failed to open {path:?}"))?;

        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .await
            .context(format!("failed to read {path:?} file"))?;

        Ok(Self::from_data(filename, data))
    }

    #[tracing::instrument(err)]
    pub async fn to_file(&self, path: &Path) -> anyhow::Result<()> {
        let manifest = self.inner.clone();
        let file = File::create(path)
            .await
            .context(format!("failed to create a {path:?} file to write manifest"))?;

        let mut writer = tokio_serde::SymmetricallyFramed::new(
            FramedWrite::new(file, LengthDelimitedCodec::new()),
            SymmetricalJson::<ManifestInner>::default(),
        );

        writer.send(manifest).await.map_err(|err| err.into())
    }

    pub fn from_data<D: AsRef<[u8]>>(filename: String, data: D) -> Self {
        let pieces_length = Self::calc_pieces_length(data.as_ref().len());
        let file_size = data.as_ref().len();
        let sharps = Self::data_to_sharps(data, pieces_length);

        let inner = ManifestInner {
            filename,
            file_size,
            piece_size: pieces_length,
            sharps,
        };

        let infohash = inner.infohash();
        Self { inner, infohash }
    }

    fn calc_pieces_length(_file_size: usize) -> usize {
        262_144
    }

    fn data_to_sharps<D: AsRef<[u8]>>(data: D, pieces_length: usize) -> Vec<Vec<u8>> {
        let mut sharps = Vec::with_capacity(pieces_length * 256);

        data.as_ref().chunks(pieces_length).into_iter().for_each(|chunk| {
            let mut hasher = Sha1::new();
            hasher.update(chunk);
            let chunk_hash = hasher.finalize().to_vec();
            sharps.push(chunk_hash);
        });

        sharps
    }

    pub fn infohash(&self) -> &[u8] {
        &self.infohash
    }

    pub fn sharps(&self) -> &[Vec<u8>] {
        &self.inner.sharps
    }

    pub fn filename(&self) -> &str {
        &self.inner.filename
    }

    pub fn piece_size(&self) -> usize {
        self.inner.piece_size
    }

    pub fn file_size(&self) -> usize {
        self.inner.file_size
    }

    pub fn pieces_count(&self) -> usize {
        self.inner.sharps.len()
    }
}

impl PartialEq for Manifest {
    fn eq(&self, other: &Self) -> bool {
        self.inner.eq(&other.inner)
    }
}

impl Hash for Manifest {
    fn hash<H: Hasher>(&self, mut state: &mut H) {
        self.inner.hash(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn different_manifests_return_different_infohashes() {
        let manifest1 = ManifestInner {
            filename: "MyFile1.txt".to_string(),
            file_size: 12,
            piece_size: 100,
            sharps: vec![vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]],
        };

        let manifest2 = ManifestInner {
            filename: "MyFile2.txt".to_string(),
            file_size: 12,
            piece_size: 100,
            sharps: vec![vec![2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2]],
        };

        assert_ne!(manifest1.infohash(), manifest2.infohash());
    }
}
