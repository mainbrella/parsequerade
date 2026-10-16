use parsequerade::{
    Client, ListingImagePlacementType, ListingImageUpload, ListingImageUploadCheckpoint,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn help() {
    println!("Upload one exact-size App Store listing PNG.\n\nUsage:\n  upload_listing_image --credentials <file> --app-id <id> --version-id <id> --locale <locale> --placement-type PRODUCT_PAGE_HEADER_ASSET|APP_STORE_SEARCH_RESULTS_ASSET --png <file> --receipt <file> [--checkpoint <file>]\n\nRead-only catalog diagnostic:\n  upload_listing_image --credentials <file> --reference-data <output.json>\n\nThe checkpoint defaults to <receipt>.checkpoint.json. Keep the checkpoint for safe recovery after an interrupted upload.");
}
fn parse_options(
    args: impl IntoIterator<Item = String>,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let mut args = args.into_iter();
    let mut out = BTreeMap::new();
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        }
        if !matches!(
            arg.as_str(),
            "--credentials"
                | "--app-id"
                | "--version-id"
                | "--locale"
                | "--placement-type"
                | "--png"
                | "--receipt"
                | "--checkpoint"
                | "--reference-data"
        ) {
            return Err("Unknown command line option.".into());
        }
        let value = args
            .next()
            .ok_or_else(|| "A command line option is missing its value.".to_string())?;
        if value.starts_with("--") || out.insert(arg, value).is_some() {
            return Err("Command line options must have one value and may not be repeated.".into());
        }
    }
    if out.contains_key("--reference-data") {
        if !out.contains_key("--credentials") || out.len() != 2 {
            return Err(
                "Reference data mode requires only --credentials and --reference-data.".into(),
            );
        }
        return Ok(Some(out));
    }
    for required in [
        "--credentials",
        "--app-id",
        "--version-id",
        "--locale",
        "--placement-type",
        "--png",
        "--receipt",
    ] {
        if !out.contains_key(required) {
            return Err("A required command line option is missing.".into());
        }
    }
    Ok(Some(out))
}
fn options() -> Result<BTreeMap<String, String>, String> {
    match parse_options(std::env::args().skip(1))? {
        Some(options) => Ok(options),
        None => {
            help();
            std::process::exit(0);
        }
    }
}
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("receipt"),
        std::process::id()
    ));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
fn bounded_read(path: &str, max: usize, message: &'static str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|_| message.to_string())?;
    let mut bytes = Vec::new();
    file.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| message.to_string())?;
    if bytes.len() > max {
        return Err(message.into());
    }
    Ok(bytes)
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let o = options()?;
    let credentials: Value = serde_json::from_slice(&bounded_read(
        &o["--credentials"],
        64 * 1024,
        "Credentials file is too large or unavailable.",
    )?)
    .map_err(|_| "Credentials file is not valid JSON.")?;
    let c = &credentials["connection"];
    let issuer = c["issuerId"]
        .as_str()
        .ok_or("Credentials are missing connection.issuerId.")?;
    let key_id = c["keyId"]
        .as_str()
        .ok_or("Credentials are missing connection.keyId.")?;
    let private_key = c["privateKey"]
        .as_str()
        .ok_or("Credentials are missing connection.privateKey.")?;
    let client = Client::new(issuer, key_id, private_key).map_err(|e| e.to_string())?;
    if let Some(output) = o.get("--reference-data") {
        let catalog = client
            .listing_image_reference_data()
            .await
            .map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(&catalog)
            .map_err(|_| "Could not serialize reference data.")?;
        atomic_write(Path::new(output), &bytes).map_err(|_| "Could not save reference data.")?;
        println!("Saved reference data.");
        return Ok(());
    }
    let receipt_path = PathBuf::from(&o["--receipt"]);
    let checkpoint_path = o
        .get("--checkpoint")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.checkpoint.json", receipt_path.display())));
    let checkpoint_parent = checkpoint_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(checkpoint_parent).map_err(|_| "Could not create checkpoint directory.")?;
    let lock_path = PathBuf::from(format!("{}.lock", checkpoint_path.display()));
    let mut lock_opts = OpenOptions::new();
    lock_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_opts.mode(0o600);
    }
    let _lock=lock_opts.open(&lock_path).map_err(|e|if e.kind()==std::io::ErrorKind::AlreadyExists { "Another upload holds the checkpoint lock. If a previous process crashed, inspect the checkpoint and remove the stale lock manually.".to_string() } else { "Could not create the checkpoint lock.".to_string() })?;
    let _guard = Lock(lock_path);
    let placement_type = match o["--placement-type"].as_str() {
        "PRODUCT_PAGE_HEADER_ASSET" => ListingImagePlacementType::ProductPageHeader,
        "APP_STORE_SEARCH_RESULTS_ASSET" => ListingImagePlacementType::SearchResults,
        _ => return Err(
            "Placement type must be PRODUCT_PAGE_HEADER_ASSET or APP_STORE_SEARCH_RESULTS_ASSET."
                .into(),
        ),
    };
    let png_path = PathBuf::from(&o["--png"]);
    let png = bounded_read(
        png_path
            .to_str()
            .ok_or("PNG path needs a valid filename.")?,
        32 * 1024 * 1024,
        "PNG file is too large or unavailable.",
    )?;
    let request = ListingImageUpload {
        app_id: o["--app-id"].clone(),
        version_id: o["--version-id"].clone(),
        locale: o["--locale"].clone(),
        placement_type,
        file_name: png_path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("PNG path needs a valid filename.")?
            .to_owned(),
    };
    let mut checkpoint = if checkpoint_path.exists() {
        serde_json::from_slice(&bounded_read(
            checkpoint_path
                .to_str()
                .ok_or("Checkpoint path is invalid.")?,
            64 * 1024,
            "Checkpoint file is too large or unavailable.",
        )?)
        .map_err(|_| "Checkpoint file is not valid JSON.")?
    } else {
        ListingImageUploadCheckpoint::default()
    };
    let result = client
        .upload_listing_image(&request, &png, &mut checkpoint, |value| {
            let bytes = serde_json::to_vec_pretty(value).map_err(|_| parsequerade::Error {
                status: 500,
                code: "checkpoint_write_failed".into(),
                message: "Could not serialize checkpoint.".into(),
                retryable: false,
                details: vec![],
                retry_after: None,
            })?;
            atomic_write(&checkpoint_path, &bytes).map_err(|_| parsequerade::Error {
                status: 500,
                code: "checkpoint_write_failed".into(),
                message: "Could not persist checkpoint.".into(),
                retryable: false,
                details: vec![],
                retry_after: None,
            })
        })
        .await
        .map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&result).map_err(|_| "Could not serialize receipt.")?;
    atomic_write(&receipt_path, &bytes).map_err(|_| "Could not persist receipt.")?;
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_options;
    use std::collections::BTreeMap;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn reference_data_mode_accepts_only_credentials_and_output() {
        let parsed = parse_options(args(&[
            "--credentials",
            "team.json",
            "--reference-data",
            "catalog.json",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            parsed,
            BTreeMap::from([
                ("--credentials".into(), "team.json".into()),
                ("--reference-data".into(), "catalog.json".into()),
            ])
        );
        assert!(parse_options(args(&[
            "--credentials",
            "team.json",
            "--reference-data",
            "catalog.json",
            "--app-id",
            "123",
        ]))
        .is_err());
        assert!(parse_options(args(&["--credentials", "team.json", "--reference-data"])).is_err());
    }

    #[test]
    fn upload_mode_still_requires_all_upload_arguments() {
        assert!(parse_options(args(&["--credentials", "team.json"])).is_err());
        assert!(parse_options(args(&[
            "--credentials",
            "team.json",
            "--app-id",
            "123",
            "--version-id",
            "version1",
            "--locale",
            "en-US",
            "--placement-type",
            "PRODUCT_PAGE_HEADER_ASSET",
            "--png",
            "header.png",
            "--receipt",
            "header.json",
        ]))
        .unwrap()
        .is_some());
    }
}
