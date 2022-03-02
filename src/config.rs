use clap::{App, Arg, ArgMatches, Values};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

const ARG_NODE_ADDRESS: &str = "address";
const ARG_UPLOAD: &str = "upload";
const ARG_DOWNLOAD: &str = "download";
pub const MANIFEST_FILE_EXT: &str = "manifest";

#[derive(Clone)]
pub struct Config {
    pub node_address: SocketAddr,
    pub upload_files: Option<Vec<PathBuf>>,
    pub manifest_files: Option<Vec<PathBuf>>,
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    fn config() -> ArgMatches {
        let file_path_validation = |path: &str| -> Result<(), String> {
            if !Path::new(path).exists() {
                Err(format!("{path} file doesn't exists"))
            } else {
                Ok(())
            }
        };

        let manifest_path_validator = |path: &str| -> Result<(), String> {
            match file_path_validation(path).map(|_| {
                if Path::new(path).extension() != Some(MANIFEST_FILE_EXT.as_ref()) {
                    Err(format!("incorrect file extension, expect: .{MANIFEST_FILE_EXT}"))
                } else {
                    Ok(())
                }
            }) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(err),
                Err(err) => Err(err),
            }
        };
        let app = App::new("p2p application")
            .arg(
                Arg::new(ARG_NODE_ADDRESS)
                    .short('a')
                    .long(ARG_NODE_ADDRESS)
                    .help("address node to be bound")
                    .takes_value(true)
                    .validator(|value| {
                        value
                            .parse::<SocketAddr>()
                            .map(|_| ())
                            .map_err(|err| format!("incorrect socket address: {}", err))
                    })
                    .required(true),
            )
            .arg(
                Arg::new(ARG_UPLOAD)
                    .short('u')
                    .long(ARG_UPLOAD)
                    .help("Specify file path to file to share")
                    .takes_value(true)
                    .multiple_values(true)
                    .validator(file_path_validation),
            )
            .arg(
                Arg::new(ARG_DOWNLOAD)
                    .short('d')
                    .long(ARG_DOWNLOAD)
                    .help("Specify manifest path to correspond file to be download")
                    .takes_value(true)
                    .multiple_values(true)
                    .validator(manifest_path_validator),
            );

        app.get_matches()
    }
}

impl Default for Config {
    fn default() -> Self {
        let config = Self::config();

        let node_address = config
            .value_of(ARG_NODE_ADDRESS)
            .map(|value| value.parse::<SocketAddr>().expect("node address is expected to valid"))
            .expect("node address is required");

        let values_to_vec_path_buf = |values: Option<Values>| -> Option<Vec<PathBuf>> {
            Some(
                values
                    .into_iter()
                    .flat_map(|value| value.into_iter().map(PathBuf::from))
                    .collect(),
            )
        };

        Self {
            node_address,
            upload_files: values_to_vec_path_buf(config.values_of(ARG_UPLOAD)),
            manifest_files: values_to_vec_path_buf(config.values_of(ARG_DOWNLOAD)),
        }
    }
}
