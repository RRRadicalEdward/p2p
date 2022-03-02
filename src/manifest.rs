use anyhow::{anyhow, Context};
use futures::{SinkExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{future::Future, path::Path, sync::Arc};
use tokio::{fs::File, io::AsyncReadExt};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

pub type ManifestsT = Arc<Manifest>;

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone, Hash)]
pub struct Manifest {
    pub filename: String,
    pub file_size: usize,
    pub pieces_length: usize,
    pub sharps: Vec<Vec<u8>>,
}

impl Manifest {
    pub async fn from_json_file<F: AsRef<Path>>(path: F) -> anyhow::Result<Self> {
        let file = File::open(path.as_ref())
            .await
            .context(format!("Failed to open {:?}", path.as_ref()))?;

        let mut reader = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(file, LengthDelimitedCodec::new()),
            SymmetricalJson::<Self>::default(),
        );

        let manifest = reader
            .try_next()
            .await
            .map(|value| value.expect("Manifest cannot be optional"))
            .map_err(|err| err.into());

        manifest
    }

    pub async fn from_path<F: AsRef<Path>>(path: F) -> anyhow::Result<Self> {
        let path = path.as_ref();

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

    pub fn to_file<F: AsRef<Path>>(&self, path: F) -> impl Future<Output = anyhow::Result<()>> {
        let manifest = self.clone();
        async move {
            let file = File::create(path.as_ref())
                .await
                .context(format!("failed to create a {:?} file to write manifest", path.as_ref()))?;

            let mut writer = tokio_serde::SymmetricallyFramed::new(
                FramedWrite::new(file, LengthDelimitedCodec::new()),
                SymmetricalJson::<Self>::default(),
            );

            writer.send(manifest).await.map_err(|err| err.into())
        }
    }

    pub fn from_data<D: AsRef<[u8]>>(filename: String, data: D) -> Self {
        let pieces_length = Self::pieces_length(data.as_ref().len());
        let file_size = data.as_ref().len();
        let sharps = Self::data_to_sharps(data, pieces_length);

        Self {
            filename,
            file_size,
            pieces_length,
            sharps,
        }
    }

    fn pieces_length(_file_size: usize) -> usize {
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

    pub fn infohash(&self) -> anyhow::Result<Vec<u8>> {
        let json_view =
            serde_json::to_vec(&self).with_context(|| "a Manifest to JSON manifest serialization failed")?;

        let mut hasher = Sha1::new();
        hasher.update(&json_view);

        Ok(hasher.finalize().to_vec())
    }

    pub fn sharps(&self) -> &[Vec<u8>] {
        &self.sharps
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }
}
