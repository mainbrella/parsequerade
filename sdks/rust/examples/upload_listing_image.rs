use parsequerade::{
    Client, ListingImagePlacementType, ListingImageUpload, ListingImageUploadCheckpoint,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

fn help() {
    println!("Upload one exact-size App Store listing PNG.\n\nUsage:\n  upload_listing_image --credentials <file> --app-id <id> --version-id <id> --locale <locale> --placement-type PRODUCT_PAGE_HEADER_ASSET|APP_STORE_SEARCH_RESULTS_ASSET --png <file> --receipt <file> [--checkpoint <file>]\n\nThe checkpoint defaults to <receipt>.checkpoint.json. Keep the checkpoint for safe recovery after an interrupted upload.");
}
fn options() -> Result<BTreeMap<String, String>, String> {
    let mut args = std::env::args().skip(1);
    let mut out = BTreeMap::new();
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            help();
            std::process::exit(0);
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
    Ok(out)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
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
    let receipt_path = PathBuf::from(&o["--receipt"]);
    let checkpoint_path = o
        .get("--checkpoint")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.checkpoint.json", receipt_path.display())));
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
    let credentials: Value = serde_json::from_slice(
        &fs::read(&o["--credentials"]).map_err(|_| "Could not read credentials file.")?,
    )
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
    let placement_type = match o["--placement-type"].as_str() {
        "PRODUCT_PAGE_HEADER_ASSET" => ListingImagePlacementType::ProductPageHeader,
        "APP_STORE_SEARCH_RESULTS_ASSET" => ListingImagePlacementType::SearchResults,
        _ => return Err(
            "Placement type must be PRODUCT_PAGE_HEADER_ASSET or APP_STORE_SEARCH_RESULTS_ASSET."
                .into(),
        ),
    };
    let png_path = PathBuf::from(&o["--png"]);
    let png = fs::read(&png_path).map_err(|_| "Could not read PNG file.")?;
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
        serde_json::from_slice(
            &fs::read(&checkpoint_path).map_err(|_| "Could not read checkpoint file.")?,
        )
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
