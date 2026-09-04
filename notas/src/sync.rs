//! Sync engine: S3-compatible object storage backend.
//!
//! The pure planning logic lives in `notas_core::sync` (see its module doc
//! for the object layout, conflict rules and tombstone semantics). This
//! module is the executor: it turns a [`SyncAction`] plan into actual S3
//! operations against the configured bucket, and updates the local
//! database accordingly. It runs on the DB worker's tokio runtime, never
//! on the UI thread.
//!
//! ## Backend selection
//!
//! - An empty endpoint uses AWS S3 (`https://s3.<region>.amazonaws.com`)
//!   with virtual-hosted addressing.
//! - A custom endpoint (Cloudflare R2, Backblaze B2, MinIO, …) uses
//!   path-style addressing, which is what the S3-compatible servers
//!   expect.
//!
//! Credentials come from the settings file, entered in the UI. The README
//! recommends a dedicated bucket-scoped access key so a leaked settings
//! file only exposes this one bucket.

use std::collections::HashMap;
use std::time::Duration;

use s3::{AddressingStyle, Auth, Client, Credentials};
use sqlx::SqlitePool;

use notas_core::repo;
use notas_core::sync::{LocalNote, RemoteEntry, Sidecar, SyncAction, content_hash, plan_sync};

use crate::config::SyncSettings;

/// Outcome of one sync run, reported to the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStats {
    /// Notes uploaded (including tombstones).
    pub uploaded: usize,
    /// Notes downloaded and applied locally.
    pub downloaded: usize,
    /// Local notes moved to the trash by a remote tombstone.
    pub trashed: usize,
    /// Conflict copies created from remote content.
    pub conflicts: usize,
    /// `YYYY-MM-DD HH:MM:SS` UTC timestamp of this run.
    pub last_synced_at: String,
}

/// Run one full sync: plan and execute, then return the stats.
pub async fn run_sync(pool: &SqlitePool, settings: &SyncSettings) -> Result<SyncStats, String> {
    // 1. Assign uuids to notes created since the last sync so every note
    //    has a stable cross-device identity.
    repo::ensure_note_uuids(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    // 2. Build the S3 client.
    let client = build_client(settings)?;
    let bucket = settings.bucket.trim().to_string();
    let prefix = normalize_prefix(&settings.prefix);

    // 3. Remote index + local index + tombstones.
    let remote = list_remote(&client, &bucket, &prefix).await?;
    let local = repo::sync_local_index(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let tombstones = repo::list_tombstones(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    // 4. Plan, then execute.
    let actions = plan_sync(&local, &tombstones, &remote);
    let mut stats = SyncStats {
        uploaded: 0,
        downloaded: 0,
        trashed: 0,
        conflicts: 0,
        last_synced_at: String::new(),
    };
    for action in actions {
        match action {
            SyncAction::Upload { note } => {
                put_note(&client, &bucket, &prefix, &note).await?;
                stats.uploaded += 1;
            }
            SyncAction::Download { sidecar } => {
                let content = get_md(&client, &bucket, &prefix, &sidecar.uuid).await?;
                repo::apply_remote_note(pool, &sidecar, &content)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                stats.downloaded += 1;
            }
            SyncAction::TrashLocal { uuid } => {
                repo::trash_note_by_uuid_no_bump(pool, &uuid)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                stats.trashed += 1;
            }
            SyncAction::ConflictCopy { sidecar } => {
                let content = get_md(&client, &bucket, &prefix, &sidecar.uuid).await?;
                repo::create_conflict_copy(pool, &sidecar, &content)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                stats.conflicts += 1;
            }
            SyncAction::UploadTombstone { uuid, deleted_at } => {
                let sidecar = Sidecar {
                    uuid,
                    title: String::new(),
                    notebook: None,
                    tags: Vec::new(),
                    trashed: false,
                    deleted: true,
                    updated_at: deleted_at,
                    content_hash: String::new(),
                };
                put_sidecar(&client, &bucket, &prefix, &sidecar).await?;
                stats.uploaded += 1;
            }
        }
    }

    // 5. Timestamp of this run, UTC (the UI shows it as-is).
    stats.last_synced_at = sqlx::query_scalar::<_, String>("SELECT datetime('now')")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    Ok(stats)
}

/// Build the S3 client from the configured endpoint/region/credentials.
///
/// An empty endpoint means AWS S3 (virtual-hosted addressing on
/// `https://s3.<region>.amazonaws.com`); a custom endpoint (R2, B2,
/// MinIO, …) switches to path-style addressing, which those
/// S3-compatible servers expect.
fn build_client(settings: &SyncSettings) -> Result<Client, String> {
    let region = settings.region.trim();
    let custom_endpoint = !settings.endpoint.trim().is_empty();
    let endpoint = if custom_endpoint {
        settings.endpoint.trim().to_string()
    } else {
        format!("https://s3.{region}.amazonaws.com")
    };
    let addressing = if custom_endpoint {
        AddressingStyle::Path
    } else {
        AddressingStyle::VirtualHosted
    };
    let auth = Auth::Static(
        Credentials::new(
            settings.access_key_id.trim(),
            settings.secret_access_key.trim(),
        )
        .map_err(|e| format!("{e:#}"))?,
    );
    Client::builder(endpoint)
        .map_err(|e| format!("{e:#}"))?
        .region(region)
        .auth(auth)
        .addressing_style(addressing)
        .timeout(Duration::from_secs(30))
        .max_attempts(3)
        .build()
        .map_err(|e| format!("{e:#}"))
}

/// Normalize the configured key prefix to `name/` form (`notas/` default).
fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        "notas/".to_string()
    } else {
        format!("{trimmed}/")
    }
}

fn md_key(prefix: &str, uuid: &str) -> String {
    format!("{prefix}notes/{uuid}.md")
}

fn meta_key(prefix: &str, uuid: &str) -> String {
    format!("{prefix}meta/{uuid}.json")
}

/// List the whole prefix and fetch every sidecar, building the remote
/// index the planner needs.
async fn list_remote(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<HashMap<String, RemoteEntry>, String> {
    let mut remote: HashMap<String, RemoteEntry> = HashMap::new();
    let mut pager = client
        .objects()
        .list_v2(bucket)
        .prefix(prefix)
        .map_err(|e| format!("{e:#}"))?
        .pager();
    while let Some(page) = pager.next_page().await.map_err(|e| format!("{e:#}"))? {
        for obj in page.contents {
            if let Some(uuid) = parse_key(prefix, &obj.key, "notes/", ".md") {
                remote.entry(uuid).or_default().has_md = true;
            } else if let Some(uuid) = parse_key(prefix, &obj.key, "meta/", ".json") {
                remote.entry(uuid).or_default();
            }
        }
    }

    // The listing carries keys but not bodies, so fetch each sidecar.
    let uuids: Vec<String> = remote.keys().cloned().collect();
    for uuid in uuids {
        let output = match client
            .objects()
            .get(bucket, meta_key(prefix, &uuid))
            .send()
            .await
        {
            Ok(output) => output,
            Err(e) => {
                // Vanished between list and fetch: leave the entry without
                // a sidecar; the planner will re-upload if a local note
                // matches, and ignore it otherwise.
                eprintln!("notas: cannot read sidecar for {uuid}: {e:#}");
                continue;
            }
        };
        let bytes = match output.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("notas: cannot read sidecar for {uuid}: {e:#}");
                continue;
            }
        };
        match serde_json::from_slice::<Sidecar>(&bytes) {
            Ok(sidecar) => {
                remote.entry(uuid).or_default().sidecar = Some(sidecar);
            }
            Err(e) => {
                eprintln!("notas: ignoring unparseable sidecar for {uuid}: {e}");
            }
        }
    }

    Ok(remote)
}

/// Split a listed key into its note uuid, if it matches `dir/…<suffix>`.
fn parse_key(prefix: &str, key: &str, dir: &str, suffix: &str) -> Option<String> {
    key.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(dir))
        .and_then(|rest| rest.strip_suffix(suffix))
        .filter(|uuid| !uuid.is_empty())
        .map(str::to_string)
}

async fn put_note(
    client: &Client,
    bucket: &str,
    prefix: &str,
    note: &LocalNote,
) -> Result<(), String> {
    let sidecar = Sidecar {
        uuid: note.uuid.clone(),
        title: note.title.clone(),
        notebook: note.notebook.clone(),
        tags: note.tags.clone(),
        trashed: note.is_trashed,
        deleted: false,
        updated_at: note.updated_at.clone(),
        content_hash: content_hash(&note.content),
    };
    client
        .objects()
        .put(bucket, md_key(prefix, &note.uuid))
        .content_type("text/markdown; charset=utf-8")
        .map_err(|e| format!("{e:#}"))?
        .body_bytes(note.content.clone())
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    put_sidecar(client, bucket, prefix, &sidecar).await
}

async fn put_sidecar(
    client: &Client,
    bucket: &str,
    prefix: &str,
    sidecar: &Sidecar,
) -> Result<(), String> {
    let json = serde_json::to_string(sidecar).map_err(|e| format!("{e:#}"))?;
    client
        .objects()
        .put(bucket, meta_key(prefix, &sidecar.uuid))
        .content_type("application/json")
        .map_err(|e| format!("{e:#}"))?
        .body_bytes(json)
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

async fn get_md(client: &Client, bucket: &str, prefix: &str, uuid: &str) -> Result<String, String> {
    let output = client
        .objects()
        .get(bucket, md_key(prefix, uuid))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let bytes = output.bytes().await.map_err(|e| format!("{e:#}"))?;
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("note {uuid} is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use notas_core::db;
    use notas_core::repo;

    /// End-to-end sync between two fresh databases through a real
    /// S3-compatible store. Manual: needs one running locally, e.g.
    ///
    /// ```bash
    /// podman run -d -p 9000:9000 \
    ///   -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
    ///   quay.io/minio/minio server /data
    /// cargo test --bin notas sync_e2e_minio -- --ignored --nocapture
    /// ```
    ///
    /// Credentials and endpoint can be overridden with
    /// `NOTAS_TEST_S3_ENDPOINT`, `NOTAS_TEST_S3_KEY` and
    /// `NOTAS_TEST_S3_SECRET`.
    #[tokio::test]
    #[ignore = "requires a running S3-compatible server (see doc comment)"]
    async fn sync_e2e_minio() {
        let endpoint = std::env::var("NOTAS_TEST_S3_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:9000".into());
        let key = std::env::var("NOTAS_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into());
        let secret = std::env::var("NOTAS_TEST_S3_SECRET").unwrap_or_else(|_| "minioadmin".into());
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let bucket = format!("notas-e2e-{unique}");

        let settings = SyncSettings {
            endpoint,
            region: "us-east-1".into(),
            bucket: bucket.clone(),
            prefix: "notas/".into(),
            access_key_id: key,
            secret_access_key: secret,
            last_synced_at: String::new(),
        };

        // Create the bucket (idempotent enough for a fresh name).
        let client = build_client(&settings).expect("client");
        client
            .buckets()
            .create(&bucket)
            .send()
            .await
            .expect("create bucket");

        // Two devices, each with its own fresh database.
        let dir = std::env::temp_dir().join(format!("notas-sync-e2e-{unique}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pool_a = db::connect(dir.join("a.db")).await.expect("db a");
        let pool_b = db::connect(dir.join("b.db")).await.expect("db b");

        // --- device A: two notes, one in a notebook, one tagged --------
        let nb = repo::create_notebook(&pool_a, None, "Work").await.unwrap();
        let n1 = repo::create_note(&pool_a, Some(nb.id), "Hello")
            .await
            .unwrap();
        repo::update_note(&pool_a, n1.id, "Hello", "body one")
            .await
            .unwrap();
        let n2 = repo::create_note(&pool_a, None, "Second").await.unwrap();
        repo::set_note_tags(&pool_a, n2.id, &["meta".to_string()])
            .await
            .unwrap();

        let stats = run_sync(&pool_a, &settings).await.expect("A sync #1");
        assert_eq!(stats.uploaded, 2, "A uploads both notes");
        assert_eq!(stats.downloaded, 0);

        // --- device B: pulls everything ----------------------------------
        let stats = run_sync(&pool_b, &settings).await.expect("B sync #1");
        assert_eq!(stats.downloaded, 2, "B downloads both notes");
        let notes_b = repo::list_all_notes(&pool_b).await.unwrap();
        assert_eq!(notes_b.len(), 2);
        let hello_b = notes_b
            .iter()
            .find(|n| n.title == "Hello")
            .expect("Hello on B");
        assert_eq!(hello_b.content, "body one");
        assert!(hello_b.notebook_id.is_some(), "note keeps its notebook");
        let tags_b = repo::get_note_tags(&pool_b, hello_b.id).await.unwrap();
        assert!(tags_b.is_empty());
        let second_b = notes_b
            .iter()
            .find(|n| n.title == "Second")
            .expect("Second on B");
        let tags_b = repo::get_note_tags(&pool_b, second_b.id).await.unwrap();
        assert_eq!(
            tags_b.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["meta"]
        );

        // --- device B edits and creates; A picks it up --------------------
        tokio::time::sleep(Duration::from_secs(1)).await;
        repo::update_note(&pool_b, hello_b.id, "Hello", "edited on B")
            .await
            .unwrap();
        repo::create_note(&pool_b, None, "From B").await.unwrap();
        let stats = run_sync(&pool_b, &settings).await.expect("B sync #2");
        assert_eq!(stats.uploaded, 2, "B uploads edit + new note");

        tokio::time::sleep(Duration::from_secs(1)).await;
        let stats = run_sync(&pool_a, &settings).await.expect("A sync #2");
        assert_eq!(stats.downloaded, 2, "A downloads B's edit + new note");
        let notes_a = repo::list_all_notes(&pool_a).await.unwrap();
        assert_eq!(notes_a.len(), 3);
        let hello_a = notes_a
            .iter()
            .find(|n| n.title == "Hello")
            .expect("Hello on A");
        assert_eq!(hello_a.content, "edited on B");

        // --- device A deletes forever; B's copy goes to the trash ---------
        tokio::time::sleep(Duration::from_secs(1)).await;
        repo::delete_note_forever(&pool_a, n2.id).await.unwrap();
        let stats = run_sync(&pool_a, &settings).await.expect("A sync #3");
        assert!(stats.uploaded >= 1, "A uploads the tombstone");

        tokio::time::sleep(Duration::from_secs(1)).await;
        let stats = run_sync(&pool_b, &settings).await.expect("B sync #3");
        assert_eq!(stats.trashed, 1, "B trashes the deleted note");
        assert_eq!(repo::list_all_notes(&pool_b).await.unwrap().len(), 2);
        assert_eq!(repo::list_trashed(&pool_b).await.unwrap().len(), 1);

        // --- a second sync is a no-op --------------------------------------
        let stats = run_sync(&pool_a, &settings).await.expect("A sync #4");
        assert_eq!(
            stats,
            SyncStats {
                uploaded: 0,
                downloaded: 0,
                trashed: 0,
                conflicts: 0,
                last_synced_at: stats.last_synced_at.clone(),
            }
        );

        pool_a.close().await;
        pool_b.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
