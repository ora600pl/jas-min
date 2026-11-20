//! Private verification metadata for resumable reads of a frozen public preview.
use super::*;
use std::io::{Read, Seek, SeekFrom};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    bytes: usize,
    sha256: String,
    blocks: Vec<String>,
}

impl Asset {
    fn measure(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.len(),
            sha256: digest(bytes),
            blocks: bytes.chunks(ASSET_CHUNK_BYTES).map(digest).collect(),
        }
    }

    fn read(&self, path: &Path, name: &str, offset: Option<&Value>) -> ToolResult<Value> {
        let (start, end) = byte_range(self.bytes, offset)?;
        let changed = || {
            tool_error(
                if name == "report.zip" {
                    "TX_EDITION_CHANGED"
                } else {
                    "TX_PREVIEW_CHANGED"
                },
                "Stored bytes differ from the reviewed client artifact.",
            )
        };
        let metadata = fs::symlink_metadata(path).map_err(io_error)?;
        if !metadata.is_file() || metadata.len() != self.bytes as u64 {
            return Err(changed());
        }
        let mut file = File::open(path).map_err(io_error)?;
        let first_block = start / ASSET_CHUNK_BYTES;
        let last_block = (end - 1) / ASSET_CHUNK_BYTES;
        let base = first_block * ASSET_CHUNK_BYTES;
        let bound = ((last_block + 1) * ASSET_CHUNK_BYTES).min(self.bytes);
        file.seek(SeekFrom::Start(base as u64)).map_err(io_error)?;
        let mut data = vec![0; bound - base];
        file.read_exact(&mut data).map_err(io_error)?;
        for (i, block) in data.chunks(ASSET_CHUNK_BYTES).enumerate() {
            if self.blocks.get(first_block + i) != Some(&digest(block)) {
                return Err(changed());
            }
        }
        Ok(json!({
            "filename":name, "content_type":public_mime(name)?, "offset":start,
            "bytes":self.bytes, "sha256":self.sha256,
            "content_base64":STANDARD.encode(&data[start-base..end-base]),
            "next_offset":(end < self.bytes).then_some(end)
        }))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    content_id: String,
    assets: BTreeMap<String, Asset>,
    archive: Asset,
}

impl Manifest {
    pub(super) fn make(files: &Files, content_id: &str, archive: &[u8]) -> ToolResult<Vec<u8>> {
        let manifest = Self {
            content_id: content_id.to_owned(),
            assets: files
                .iter()
                .map(|(n, data)| (n.clone(), Asset::measure(data)))
                .collect(),
            archive: Asset::measure(archive),
        };
        serde_json::to_vec(&manifest).map_err(io_error)
    }

    pub(super) fn preview_id(recipe: &[u8], manifest: &[u8]) -> String {
        let mut hash = Sha256::new();
        hash.update(Sha256::digest(manifest));
        hash.update(recipe);
        format!("{:x}", hash.finalize())
    }

    pub(super) fn read<R: PublicationRecipe>(
        folder: &Path,
        workspace: &Path,
        preview_id: &str,
        recipe: &[u8],
        args: &Map<String, Value>,
    ) -> ToolResult<Value> {
        let contents = regular_bytes(&folder.join("assets.json")).map_err(io_error)?;
        if Self::preview_id(recipe, &contents) != preview_id {
            return Err(tool_error(
                "TX_PREVIEW_CHANGED",
                "The saved asset manifest differs from the reviewed preview.",
            ));
        }
        let manifest: Self = serde_json::from_slice(&contents).map_err(io_error)?;
        let name = args.get("asset").and_then(Value::as_str).unwrap_or("");
        let offset = args.get("offset");
        if name == "report.zip" {
            if !has_publication_receipt(
                folder,
                preview_id,
                &manifest.content_id,
                &manifest.archive.sha256,
            ) {
                return Err(tool_error(
                    "TX_ZIP_NOT_PUBLISHED",
                    "Publish this exact preview before downloading its ZIP.",
                ));
            }
            let dir = existing_directories(workspace, &["wydania", &manifest.content_id])?;
            return manifest
                .archive
                .read(&dir.join(R::ZIP_FILENAME), name, offset);
        }
        let file = manifest
            .assets
            .get(name)
            .ok_or_else(|| invalid("Asset is outside the public report."))?;
        validate_public_path(name)?;
        let public = existing_directories(folder, &["public"])?;
        let path = match name.split_once('/') {
            Some((subdir, filename)) => existing_directories(&public, &[subdir])?.join(filename),
            None => public.join(name),
        };
        file.read(&path, name, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_reads_verify_full_blocks_at_unaligned_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plotly.min.js");
        let mut bytes: Vec<_> = (0..ASSET_CHUNK_BYTES * 2 + 17)
            .map(|n| (n % 251) as u8)
            .collect();
        fs::write(&path, &bytes).unwrap();
        let asset = Asset::measure(&bytes);
        let start = ASSET_CHUNK_BYTES - 3;
        let chunk = asset
            .read(&path, "plotly.min.js", Some(&json!(start)))
            .unwrap();
        assert_eq!(chunk["next_offset"], start + ASSET_CHUNK_BYTES);
        assert_eq!(chunk["sha256"], digest(&bytes));
        assert_eq!(
            STANDARD
                .decode(chunk["content_base64"].as_str().unwrap())
                .unwrap(),
            bytes[start..start + ASSET_CHUNK_BYTES]
        );
        bytes[ASSET_CHUNK_BYTES + 19] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            asset
                .read(&path, "plotly.min.js", Some(&json!(start)))
                .unwrap_err()["error_code"],
            "TX_PREVIEW_CHANGED"
        );
        assert!(asset.read(&path, "plotly.min.js", Some(&json!(0))).is_ok());
        bytes.pop();
        fs::write(&path, &bytes).unwrap();
        assert!(asset.read(&path, "plotly.min.js", Some(&json!(0))).is_err());
    }

    #[test]
    fn public_paths_and_preview_checks_accept_only_the_known_directory_and_reviewed_files() {
        for name in [
            "/index.html",
            "../index.html",
            "wykresy/../index.html",
            "wykresy//chart.html",
            "wykresy/.chart.html",
            "wykresy/nested/chart.html",
            "wykresy/report.zip",
            "wykresy/notes.json",
            "private/report.css",
            "report.zip",
        ] {
            assert!(validate_public_path(name).is_err(), "{name}");
        }
        let dir = tempfile::tempdir().unwrap();
        let file = "wykresy/tx-chart-1.html";
        let flat = "index.html";
        let files = BTreeMap::from([
            (flat.into(), b"report".to_vec()),
            (file.into(), b"chart".to_vec()),
        ]);
        let recipe = b"test recipe";
        let manifest = Manifest::make(&files, &hash_files(&files), b"archive").unwrap();
        let id = Manifest::preview_id(recipe, &manifest);
        let folder = dir.path().join(&id);
        for name in files.keys() {
            validate_public_path(name).unwrap();
        }
        prepare(&folder, recipe, &files, Some(&manifest)).unwrap();
        let public = folder.join("public");
        let read = || {
            Manifest::read::<Recipe>(
                &folder,
                dir.path(),
                &id,
                recipe,
                json!({"asset":file}).as_object().unwrap(),
            )
        };
        assert_eq!(read().unwrap()["sha256"], digest(b"chart"));
        let unexpected = public.join("wykresy/unlisted.html");
        fs::write(&unexpected, b"unreviewed").unwrap();
        assert!(checked_files(&public, &files).is_err());
        fs::remove_file(unexpected).unwrap();
        assert!(checked_files(&public, &files).is_ok());

        #[cfg(unix)]
        {
            let original = public.join(CHART_DIRECTORY);
            let alternate = dir.path().join("moved");
            fs::rename(&original, &alternate).unwrap();
            std::os::unix::fs::symlink(&alternate, &original).unwrap();
            assert!(read().is_err());
            assert!(checked_files(&public, &files).is_err());
        }
    }

    #[test]
    fn manifest_is_bound_to_the_preview_and_its_private_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let files = BTreeMap::from([("index.html".to_owned(), b"public".to_vec())]);
        let recipe = b"test recipe";
        let manifest = Manifest::make(&files, &hash_files(&files), b"archive").unwrap();
        let id = Manifest::preview_id(recipe, &manifest);
        let public = dir.path().join("public");
        fs::create_dir(&public).unwrap();
        fs::write(public.join("index.html"), b"public").unwrap();
        let manifest_file = dir.path().join("assets.json");
        fs::write(&manifest_file, &manifest).unwrap();
        let args = json!({"asset":"index.html"}).as_object().unwrap().clone();
        assert!(Manifest::read::<Recipe>(dir.path(), dir.path(), &id, recipe, &args).is_ok());
        assert!(
            Manifest::read::<Recipe>(dir.path(), dir.path(), &id, b"different recipe", &args)
                .is_err()
        );
        let mut forged: Value = serde_json::from_slice(&manifest).unwrap();
        forged["assets"]["index.html"]["bytes"] = 0.into();
        fs::write(manifest_file, forged.to_string()).unwrap();
        assert!(Manifest::read::<Recipe>(dir.path(), dir.path(), &id, recipe, &args).is_err());
    }
}
