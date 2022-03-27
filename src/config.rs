use clap::Parser;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

const ARG_NODE_ADDRESS: &str = "address";
const ARG_UPLOAD: &str = "upload";
const ARG_DOWNLOAD: &str = "download";
pub const MANIFEST_FILE_EXT: &str = "manifest";

#[derive(Parser)]
#[clap(name = "p2p application")]
pub struct Config {
    #[clap(name = ARG_NODE_ADDRESS,short = 'a', long = ARG_NODE_ADDRESS, parse(try_from_str), required = true, help = "Address the node to be bound to")]
    pub node_address: SocketAddr,
    #[clap(name = ARG_UPLOAD, short = 'u', long = ARG_UPLOAD, takes_value = true, multiple_values = true, validator = file_path_validation, help = "Specify file path to file to share")]
    pub upload_files: Option<Vec<PathBuf>>,
    #[clap(name = ARG_DOWNLOAD, short = 'd', long = ARG_DOWNLOAD, takes_value = true, multiple_values = true, validator = manifest_path_validator, help = "Specify manifest path to correspond file to be download")]
    pub manifest_files: Option<Vec<PathBuf>>,
}

fn file_path_validation(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.exists() {
        Err(format!("{path:?} file doesn't exists"))
    } else if !path.is_file() {
        Err(format!("{path:?} is not a file"))
    } else {
        Ok(())
    }
}

fn manifest_path_validator(path: &str) -> Result<(), String> {
    match file_path_validation(path).map(|_| {
        if Path::new(path).extension() != Some(MANIFEST_FILE_EXT.as_ref()) {
            Err(format!(
                "`{path}` has incorrect file extension, expect: .{MANIFEST_FILE_EXT}"
            ))
        } else {
            Ok(())
        }
    }) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}
