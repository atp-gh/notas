//! Sync engine: S3-compatible object storage and WebDAV backends.
//!
//! The pure planning logic lives in `notas_core::sync` (see its module doc
//! for the object layout, conflict rules and tombstone semantics). This
//! module is the executor: each backend implements the [`SyncStore`] seam
//! — list the remote index, fetch/upload markdown bodies and sidecars —
//! and a shared [`run_sync_with`] turns a [`SyncAction`] plan into actual
//! store operations, then updates the local database. It runs on the DB
//! worker's tokio runtime, never on the UI thread.
//!
//! ## Backends
//!
//! - **S3** (`S3Store`): an empty endpoint uses AWS S3
//!   (`https://s3.<region>.amazonaws.com`) with virtual-hosted addressing;
//!   a custom endpoint (Cloudflare R2, Backblaze B2, MinIO, …) uses
//!   path-style addressing.
//! - **WebDAV** (`WebDavStore`): RFC-4918 collections via
//!   `reqwest_dav`, Basic auth by default. The configured URL points at a
//!   writable DAV collection; the notes live under
//!   `notes/<uuid>.md` and `meta/<uuid>.json` inside it (or inside the
//!   configured `directory`), mirroring the S3 object layout so the
//!   planner and its conflict rules are identical for both backends.
//!
//! Credentials come from the settings file, entered in the UI. The README
//! recommends scoped credentials: a bucket-only S3 access key, or a
//! Nextcloud *app password*.

use std::collections::HashMap;
use std::time::Duration;

use reqwest_dav::{Auth as DavAuth, Client as DavClient, ClientBuilder as DavClientBuilder, Depth};
use s3::{AddressingStyle, Auth as S3Auth, Client as S3Client, Credentials};
use sqlx::SqlitePool;

use notas_core::repo;
use notas_core::sync::{LocalNote, RemoteEntry, Sidecar, SyncAction, content_hash, plan_sync};

use crate::config::{S3SyncSettings, SyncSettings, SyncType, WebDavSyncSettings};

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
    /// `YYYY-MM-DD HH:MM:SS` timestamp of this run, in the device's local
    /// time — it exists only to be shown in the UI.
    pub last_synced_at: String,
}

/// The storage primitives the planner's actions map onto. Implemented by
/// every sync backend; the rest of [`run_sync_with`] is shared.
trait SyncStore {
    /// List the whole remote store, fetching every sidecar, and build the
    /// remote index the planner needs.
    async fn list(&self) -> Result<HashMap<String, RemoteEntry>, String>;
    /// Fetch the markdown body of a note.
    async fn get_md(&self, uuid: &str) -> Result<String, String>;
    /// Upload a note's markdown body and sidecar.
    async fn put_note(&self, note: &LocalNote) -> Result<(), String>;
    /// Upload a sidecar alone (used for tombstones).
    async fn put_sidecar(&self, sidecar: &Sidecar) -> Result<(), String>;
}

/// Run one full sync against the configured backend, then return stats.
pub async fn run_sync(pool: &SqlitePool, settings: &SyncSettings) -> Result<SyncStats, String> {
    match settings.kind {
        SyncType::S3 => {
            let store = S3Store::new(&settings.s3)?;
            run_sync_with(pool, &store).await
        }
        SyncType::WebDAV => {
            let store = WebDavStore::new(&settings.webdav)?;
            run_sync_with(pool, &store).await
        }
    }
}

/// Plan and execute one sync against any [`SyncStore`]: build both
/// indexes, run the pure planner, execute every action, and record the
/// run's timestamp.
async fn run_sync_with(pool: &SqlitePool, store: &impl SyncStore) -> Result<SyncStats, String> {
    // 1. Assign uuids to notes created since the last sync so every note
    //    has a stable cross-device identity.
    repo::ensure_note_uuids(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    // 2. Remote index + local index + tombstones.
    let remote = store.list().await?;
    let local = repo::sync_local_index(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let tombstones = repo::list_tombstones(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    // 3. Plan, then execute.
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
                store.put_note(&note).await?;
                stats.uploaded += 1;
            }
            SyncAction::Download { sidecar } => {
                let content = store.get_md(&sidecar.uuid).await?;
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
                let content = store.get_md(&sidecar.uuid).await?;
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
                store.put_sidecar(&sidecar).await?;
                stats.uploaded += 1;
            }
        }
    }

    // 4. Timestamp of this run, in the device's local time (display only;
    //    note timestamps themselves stay UTC in the database).
    stats.last_synced_at = sqlx::query_scalar::<_, String>("SELECT datetime('now', 'localtime')")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("{e:#}"))?;

    Ok(stats)
}

// ------------------------------------------------------------- S3 backend

/// S3-compatible object store.
struct S3Store {
    client: S3Client,
    bucket: String,
    prefix: String,
}

impl S3Store {
    /// Build the client from the S3 settings.
    fn new(settings: &S3SyncSettings) -> Result<Self, String> {
        let client = build_client(settings)?;
        let bucket = settings.bucket.trim().to_string();
        let prefix = normalize_prefix(&settings.prefix);
        Ok(Self {
            client,
            bucket,
            prefix,
        })
    }
}

impl SyncStore for S3Store {
    async fn list(&self) -> Result<HashMap<String, RemoteEntry>, String> {
        list_remote(&self.client, &self.bucket, &self.prefix).await
    }

    async fn get_md(&self, uuid: &str) -> Result<String, String> {
        get_md(&self.client, &self.bucket, &self.prefix, uuid).await
    }

    async fn put_note(&self, note: &LocalNote) -> Result<(), String> {
        put_note(&self.client, &self.bucket, &self.prefix, note).await
    }

    async fn put_sidecar(&self, sidecar: &Sidecar) -> Result<(), String> {
        put_sidecar(&self.client, &self.bucket, &self.prefix, sidecar).await
    }
}

/// Build the S3 client from the configured endpoint/region/credentials.
///
/// An empty endpoint means AWS S3 (virtual-hosted addressing on
/// `https://s3.<region>.amazonaws.com`); a custom endpoint (R2, B2,
/// MinIO, …) switches to path-style addressing, which those
/// S3-compatible servers expect.
fn build_client(settings: &S3SyncSettings) -> Result<S3Client, String> {
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
    let auth = S3Auth::Static(
        Credentials::new(
            settings.access_key_id.trim(),
            settings.secret_access_key.trim(),
        )
        .map_err(|e| format!("{e:#}"))?,
    );
    S3Client::builder(endpoint)
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
    client: &S3Client,
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
    client: &S3Client,
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
    client: &S3Client,
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

async fn get_md(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
    uuid: &str,
) -> Result<String, String> {
    let output = client
        .objects()
        .get(bucket, md_key(prefix, uuid))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let bytes = output.bytes().await.map_err(|e| format!("{e:#}"))?;
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("note {uuid} is not valid UTF-8: {e}"))
}

// --------------------------------------------------------- WebDAV backend

/// WebDAV (RFC-4918) collection store, using `reqwest_dav` over a
/// hand-built `reqwest` agent.
///
/// The client host is the configured collection URL, so all paths below
/// are relative to it: `notes/`, `meta/`, `notes/<uuid>.md`,
/// `meta/<uuid>.json` (optionally prefixed by the configured
/// `directory`). The server decides the exact href form returned by
/// PROPFIND — absolute path, full URL, … — so uuid extraction only looks
/// at the last path segment.
struct WebDavStore {
    client: DavClient,
    /// Configured subfolder under the URL, trimmed of slashes (empty when
    /// the notes live directly in the URL).
    directory: String,
}

impl WebDavStore {
    /// Build the store from the WebDAV settings. `url` must be https (or
    /// plain http when the user opted into insecure TLS).
    fn new(settings: &WebDavSyncSettings) -> Result<Self, String> {
        let url = webdav_base_url(settings)?;
        let agent = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(settings.insecure_tls)
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {e}"))?;
        // Empty username means an anonymous server; otherwise Basic auth.
        let auth = if settings.username.trim().is_empty() {
            DavAuth::Anonymous
        } else {
            DavAuth::Basic(settings.username.trim().into(), settings.password.clone())
        };
        let client = DavClientBuilder::new()
            .set_agent(agent)
            .set_host(url)
            .set_auth(auth)
            .build()
            .map_err(|e| format!("cannot build the WebDAV client: {e}"))?;
        let directory = settings.directory.trim().trim_matches('/').to_string();
        Ok(Self { client, directory })
    }

    /// Path of one notes/meta sub-collection relative to the host.
    fn collection_path(&self, dir: &str, child: &str) -> String {
        if dir.is_empty() {
            child.to_string()
        } else {
            format!("{dir}/{child}")
        }
    }

    fn notes_collection(&self) -> String {
        self.collection_path(&self.directory, "notes")
    }

    fn meta_collection(&self) -> String {
        self.collection_path(&self.directory, "meta")
    }

    /// Create the base collection and its two sub-collections if missing.
    /// MKCOL on an existing collection is refused (405) by most servers,
    /// so "already there" statuses count as success.
    async fn ensure_collections(&self) -> Result<(), String> {
        // The URL itself is expected to exist; only an explicit directory
        // is created. Without one, the notes go straight into the URL.
        if !self.directory.is_empty() {
            self.ensure_collection(&self.directory).await?;
        }
        self.ensure_collection(&self.notes_collection()).await?;
        self.ensure_collection(&self.meta_collection()).await
    }

    async fn ensure_collection(&self, path: &str) -> Result<(), String> {
        let response = self.client.mkcol_raw(path).await.map_err(webdav_error)?;
        let code = response.status().as_u16();
        if collection_ok(code) {
            Ok(())
        } else {
            Err(webdav_status_error("create collection", code))
        }
    }

    /// PROPFIND one collection (depth 1) and return the uuids of the
    /// files in it, identified by their `suffix` (`.md`/`.json`).
    async fn list_collection(&self, collection: &str, suffix: &str) -> Result<Vec<String>, String> {
        let responses = self
            .client
            .list_rsp(collection, Depth::Number(1))
            .await
            .map_err(webdav_error)?;
        Ok(responses
            .iter()
            .filter_map(|r| uuid_from_href(&r.href, suffix))
            .collect())
    }

    /// GET one sidecar; `None` when the server says 404 (it vanished
    /// between listing and fetching, or the file is foreign).
    async fn get_sidecar(&self, uuid: &str) -> Result<Option<Sidecar>, String> {
        let path = format!("{}/{uuid}.json", self.meta_collection());
        let response = self.client.get_raw(&path).await.map_err(webdav_error)?;
        let code = response.status().as_u16();
        if code == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(webdav_status_error("read sidecar", code));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("cannot read sidecar for {uuid}: {e}"))?;
        match serde_json::from_slice::<Sidecar>(&bytes) {
            Ok(sidecar) => Ok(Some(sidecar)),
            Err(e) => {
                eprintln!("notas: ignoring unparseable sidecar for {uuid}: {e}");
                Ok(None)
            }
        }
    }

    /// PUT one file with an explicit content type.
    async fn put(&self, path: &str, content_type: &str, body: String) -> Result<(), String> {
        let builder = self
            .client
            .start_request(reqwest::Method::PUT, path)
            .await
            .map_err(webdav_error)?;
        let response = builder
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("cannot upload: {e}"))?;
        let code = response.status().as_u16();
        if response.status().is_success() {
            Ok(())
        } else {
            Err(webdav_status_error("upload", code))
        }
    }
}

impl SyncStore for WebDavStore {
    async fn list(&self) -> Result<HashMap<String, RemoteEntry>, String> {
        self.ensure_collections().await?;

        let mut remote: HashMap<String, RemoteEntry> = HashMap::new();
        // Same shape as the S3 index: the notes collection marks `has_md`,
        // the meta collection creates the entry, then sidecars are read.
        for uuid in self
            .list_collection(&self.notes_collection(), ".md")
            .await?
        {
            remote.entry(uuid).or_default().has_md = true;
        }
        for uuid in self
            .list_collection(&self.meta_collection(), ".json")
            .await?
        {
            remote.entry(uuid).or_default();
        }

        let uuids: Vec<String> = remote.keys().cloned().collect();
        for uuid in uuids {
            match self.get_sidecar(&uuid).await {
                Ok(Some(sidecar)) => {
                    remote.entry(uuid).or_default().sidecar = Some(sidecar);
                }
                Ok(None) => {
                    // Vanished or unparseable: leave the entry without a
                    // sidecar; the planner re-uploads if a local note
                    // matches, and ignores it otherwise.
                }
                Err(e) => eprintln!("notas: cannot read sidecar for {uuid}: {e}"),
            }
        }
        Ok(remote)
    }

    async fn get_md(&self, uuid: &str) -> Result<String, String> {
        let path = format!("{}/{uuid}.md", self.notes_collection());
        let response = self.client.get_raw(&path).await.map_err(webdav_error)?;
        let code = response.status().as_u16();
        if !response.status().is_success() {
            return Err(webdav_status_error(&format!("read note {uuid}"), code));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("cannot read note {uuid}: {e}"))?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| format!("note {uuid} is not valid UTF-8: {e}"))
    }

    async fn put_note(&self, note: &LocalNote) -> Result<(), String> {
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
        let md_path = format!("{}/{}.md", self.notes_collection(), note.uuid);
        self.put(
            &md_path,
            "text/markdown; charset=utf-8",
            note.content.clone(),
        )
        .await?;
        self.put_sidecar(&sidecar).await
    }

    async fn put_sidecar(&self, sidecar: &Sidecar) -> Result<(), String> {
        let path = format!("{}/{}.json", self.meta_collection(), sidecar.uuid);
        let json = serde_json::to_string(sidecar).map_err(|e| format!("{e:#}"))?;
        self.put(&path, "application/json", json).await
    }
}

/// Resolve the configured URL into the base the WebDAV client talks to.
///
/// A missing scheme defaults to `https://`; plain `http://` is only
/// accepted when the user opted into insecure TLS.
fn webdav_base_url(settings: &WebDavSyncSettings) -> Result<String, String> {
    let raw = settings.url.trim();
    if raw.is_empty() {
        return Err("WebDAV URL is empty — set the server URL in Settings".to_string());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let parsed = reqwest::Url::parse(&with_scheme)
        .map_err(|e| format!("WebDAV URL \"{raw}\" is not a valid URL: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if settings.insecure_tls => {}
        "http" => {
            return Err(
                "WebDAV URL uses plain http — enable “allow insecure TLS” to accept it".to_string(),
            );
        }
        other => {
            return Err(format!(
                "WebDAV URL scheme \"{other}\" is not supported — use https"
            ));
        }
    }
    Ok(with_scheme.trim_end_matches('/').to_string())
}

/// Extract the uuid of a note file from a PROPFIND href. Servers return
/// hrefs in wildly different forms (absolute path, full URL, trailing
/// slash), so only the last path segment matters.
fn uuid_from_href(href: &str, suffix: &str) -> Option<String> {
    let name = href.trim_end_matches('/').rsplit('/').next()?;
    name.strip_suffix(suffix)
        .filter(|uuid| !uuid.is_empty())
        .map(str::to_string)
}

/// Statuses that mean "the collection exists now": created (2xx), moved
/// (301/302, servers that redirect bare MKCOLs), or refused because it is
/// already there (405, the standard answer).
fn collection_ok(code: u16) -> bool {
    code / 100 == 2 || matches!(code, 301 | 302 | 405)
}

/// Map a `reqwest_dav` error to a human-readable string, extracting the
/// HTTP status where the crate wrapped it.
fn webdav_error(err: reqwest_dav::Error) -> String {
    use reqwest_dav::DecodeError;
    match &err {
        reqwest_dav::Error::Decode(DecodeError::StatusMismatched(status)) => {
            webdav_status_error("request", status.response_code)
        }
        reqwest_dav::Error::Decode(DecodeError::Server(server)) => {
            webdav_status_error("request", server.response_code)
        }
        reqwest_dav::Error::Reqwest(e) => format!("WebDAV network error: {e}"),
        other => format!("WebDAV error: {other}"),
    }
}

/// Friendly message for a failed WebDAV operation with its HTTP status.
fn webdav_status_error(operation: &str, code: u16) -> String {
    match code {
        401 | 403 => format!(
            "WebDAV {operation}: the server rejected the credentials (HTTP {code}) — \
             check the username and password; for Nextcloud use an app password"
        ),
        404 => format!(
            "WebDAV {operation}: not found (HTTP 404) — check the server URL and folder path"
        ),
        405 => format!(
            "WebDAV {operation}: method not allowed (HTTP 405) — the URL may not be a WebDAV endpoint"
        ),
        409 => format!(
            "WebDAV {operation}: conflict (HTTP 409) — a parent folder is missing on the server"
        ),
        _ => format!("WebDAV {operation} failed (HTTP {code})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------- pure helpers

    #[test]
    fn uuid_from_href_handles_any_href_form() {
        // Absolute path, full URL and trailing-slash variants.
        for href in [
            "/notas/notes/01234567-89ab-4cde-8f01-23456789abcd.md",
            "https://nc.example/remote.php/dav/files/alice/notas/notes/01234567-89ab-4cde-8f01-23456789abcd.md",
            "01234567-89ab-4cde-8f01-23456789abcd.md",
        ] {
            assert_eq!(
                uuid_from_href(href, ".md").as_deref(),
                Some("01234567-89ab-4cde-8f01-23456789abcd")
            );
        }
        // Folders (trailing slash, no suffix) never match; the suffix must
        // match the collection being listed. Any other `.md` name maps to
        // an opaque uuid — the planner tolerates foreign files the same
        // way it tolerates an orphan object in the S3 bucket.
        assert_eq!(uuid_from_href("/notas/notes/", ".md"), None);
        assert_eq!(
            uuid_from_href("/notas/notes/readme.md", ".md").as_deref(),
            Some("readme")
        );
        assert_eq!(uuid_from_href("/notas/meta/x.json", ".md"), None);
    }

    fn webdav_settings(url: &str) -> WebDavSyncSettings {
        WebDavSyncSettings {
            url: url.into(),
            username: "alice".into(),
            password: "secret".into(),
            directory: "notas".into(),
            insecure_tls: true,
        }
    }

    #[test]
    fn webdav_url_defaults_to_https_and_requires_insecure_for_http() {
        let ok =
            webdav_base_url(&webdav_settings("nc.example/remote.php/dav/files/alice")).unwrap();
        assert_eq!(ok, "https://nc.example/remote.php/dav/files/alice");

        // Plain http is refused unless the user opted into insecure TLS.
        let mut plain = WebDavSyncSettings {
            insecure_tls: false,
            ..webdav_settings("http://192.168.1.10/webdav")
        };
        let err = webdav_base_url(&plain).unwrap_err();
        assert!(err.contains("insecure"), "{err}");
        plain.insecure_tls = true;
        assert_eq!(
            webdav_base_url(&plain).unwrap(),
            "http://192.168.1.10/webdav"
        );

        let err = webdav_base_url(&webdav_settings("ftp://example.com")).unwrap_err();
        assert!(err.contains("https"), "{err}");

        let mut empty = webdav_settings("");
        empty.url.clear();
        assert!(webdav_base_url(&empty).unwrap_err().contains("empty"));
    }

    #[test]
    fn collection_ok_treats_existing_collections_as_fine() {
        assert!(collection_ok(201));
        assert!(collection_ok(405));
        assert!(collection_ok(301));
        assert!(!collection_ok(404));
        assert!(!collection_ok(401));
        assert!(!collection_ok(500));
    }
}

// -------------------------------------------------- WebDAV mock integration

/// End-to-end-ish executor tests against an in-process WebDAV mock
/// (`wiremock`): real PROPFIND/PUT/GET traffic over HTTP to `127.0.0.1`,
/// driving the real planner and a real database.
#[cfg(test)]
mod webdav_tests {
    use super::*;
    use wiremock::matchers::{basic_auth, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use notas_core::db;

    /// Test settings pointing at the mock server (plain http, so the
    /// insecure-TLS option is on).
    fn webdav_settings(url: &str) -> WebDavSyncSettings {
        WebDavSyncSettings {
            url: url.into(),
            username: "alice".into(),
            password: "secret".into(),
            directory: "notas".into(),
            insecure_tls: true,
        }
    }

    /// Fresh temp directory for one test's databases.
    fn temp_db_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("notas-webdav-{tag}-{}-{nanos}", std::process::id()))
    }

    /// A PROPFIND 207 multistatus body listing `hrefs` as files. hrefs are
    /// returned verbatim; the executor only reads the last path segment.
    fn multistatus(hrefs: &[String]) -> String {
        let mut body = String::from(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <D:multistatus xmlns:D=\"DAV:\">",
        );
        for href in hrefs {
            body.push_str(&format!(
                "<D:response><D:href>{href}</D:href>\
                 <D:propstat><D:status>HTTP/1.1 200 OK</D:status>\
                 <D:prop><D:getcontenttype>text/plain</D:getcontenttype>\
                 <D:getcontentlength>1</D:getcontentlength></D:prop></D:propstat>\
                 </D:response>"
            ));
        }
        body.push_str("</D:multistatus>");
        body
    }

    /// A PROPFIND answer for an empty collection: the server lists the
    /// collection itself (a folder), which the executor ignores.
    fn empty_multistatus(collection_href: &str) -> String {
        multistatus(&[format!("{collection_href}/")])
    }

    /// Accept MKCOL on anything (base + sub-collections): the store treats
    /// "already there" as success, so 405 is a neutral answer.
    async fn accept_mkcol(server: &MockServer) {
        Mock::given(method("MKCOL"))
            .respond_with(ResponseTemplate::new(405))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn webdav_executor_uploads_a_new_note() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        Mock::given(method("PROPFIND"))
            .and(path("/notas/notes"))
            .respond_with(
                ResponseTemplate::new(207).set_body_string(empty_multistatus("/notas/notes")),
            )
            .mount(&server)
            .await;
        Mock::given(method("PROPFIND"))
            .and(path("/notas/meta"))
            .respond_with(
                ResponseTemplate::new(207).set_body_string(empty_multistatus("/notas/meta")),
            )
            .mount(&server)
            .await;

        let dir = temp_db_dir("upload");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let note = repo::create_note(&pool, None, "Hello").await.unwrap();
        repo::update_note(&pool, note.id, "Hello", "body one")
            .await
            .unwrap();
        repo::ensure_note_uuids(&pool).await.unwrap();
        let uuid = repo::sync_local_index(&pool).await.unwrap()[0].uuid.clone();

        // The two PUTs the upload performs, with the right content types.
        let md_path = format!("/notas/notes/{uuid}.md");
        let meta_path = format!("/notas/meta/{uuid}.json");
        Mock::given(method("PUT"))
            .and(path(md_path))
            .and(header("content-type", "text/markdown; charset=utf-8"))
            .and(basic_auth("alice", "secret"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(meta_path))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            last_synced_at: String::new(),
        };
        let stats = run_sync(&pool, &settings).await.expect("webdav sync");
        assert_eq!(stats.uploaded, 1);
        // The two PUTs happened exactly once, with Basic auth headers.
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_downloads_a_remote_note() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let md_href = format!("/notas/notes/{uuid}.md");
        let meta_href = format!("/notas/meta/{uuid}.json");

        Mock::given(method("PROPFIND"))
            .and(path("/notas/notes"))
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus(std::slice::from_ref(&md_href))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PROPFIND"))
            .and(path("/notas/meta"))
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus(std::slice::from_ref(&meta_href))),
            )
            .mount(&server)
            .await;

        let body = "remote body".to_string();
        let sidecar = Sidecar {
            uuid: uuid.clone(),
            title: "Remote".into(),
            notebook: None,
            tags: vec!["work".into()],
            trashed: false,
            deleted: false,
            updated_at: "2026-01-02 03:04:05".into(),
            content_hash: content_hash(&body),
        };
        let sidecar_json = serde_json::to_string(&sidecar).unwrap();

        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{uuid}.json")))
            .respond_with(ResponseTemplate::new(200).set_body_string(sidecar_json))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/notes/{uuid}.md")))
            .and(basic_auth("alice", "secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.clone()))
            .mount(&server)
            .await;

        let dir = temp_db_dir("download");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            last_synced_at: String::new(),
        };
        let stats = run_sync(&pool, &settings).await.expect("webdav sync");
        assert_eq!(stats.downloaded, 1);

        let notes = repo::list_all_notes(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Remote");
        assert_eq!(notes[0].content, "remote body");
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_trashes_a_note_honouring_a_tombstone() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();

        // Remote holds only a tombstone (deleted sidecar, no md).
        Mock::given(method("PROPFIND"))
            .and(path("/notas/notes"))
            .respond_with(
                ResponseTemplate::new(207).set_body_string(empty_multistatus("/notas/notes")),
            )
            .mount(&server)
            .await;
        let meta_href = format!("/notas/meta/{uuid}.json");
        Mock::given(method("PROPFIND"))
            .and(path("/notas/meta"))
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus(std::slice::from_ref(&meta_href))),
            )
            .mount(&server)
            .await;
        let tombstone = Sidecar {
            uuid: uuid.clone(),
            title: String::new(),
            notebook: None,
            tags: Vec::new(),
            trashed: false,
            deleted: true,
            updated_at: "2099-01-01 00:00:00".into(),
            content_hash: String::new(),
        };
        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{uuid}.json")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(serde_json::to_string(&tombstone).unwrap()),
            )
            .mount(&server)
            .await;

        let dir = temp_db_dir("trash");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        // A local note with the tombstone's uuid that predates it (old
        // timestamp): the planner must trash it locally, not re-upload.
        let note = repo::create_note(&pool, None, "Old").await.unwrap();
        repo::update_note(&pool, note.id, "Old", "old body")
            .await
            .unwrap();
        repo::ensure_note_uuids(&pool).await.unwrap();
        sqlx::query("UPDATE notes SET uuid = ?1, updated_at = '2000-01-01 00:00:00' WHERE id = ?2")
            .bind(&uuid)
            .bind(note.id)
            .execute(&pool)
            .await
            .unwrap();

        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            last_synced_at: String::new(),
        };
        let stats = run_sync(&pool, &settings).await.expect("webdav sync");
        assert_eq!(stats.trashed, 1);
        assert_eq!(repo::list_all_notes(&pool).await.unwrap().len(), 0);
        assert_eq!(repo::list_trashed(&pool).await.unwrap().len(), 1);
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_reports_bad_credentials() {
        let server = MockServer::start().await;
        // The very first MKCOL is refused with 401.
        Mock::given(method("MKCOL"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let dir = temp_db_dir("auth");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            last_synced_at: String::new(),
        };
        let err = run_sync(&pool, &settings).await.unwrap_err();
        assert!(err.contains("401"), "{err}");
        assert!(err.contains("app password"), "{err}");
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ------------------------------------------------- S3 two-device e2e (manual)

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use notas_core::db;

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

        let s3 = S3SyncSettings {
            endpoint,
            region: "us-east-1".into(),
            bucket: bucket.clone(),
            prefix: "notas/".into(),
            access_key_id: key,
            secret_access_key: secret,
        };
        let settings = SyncSettings {
            kind: SyncType::S3,
            s3,
            webdav: WebDavSyncSettings::default(),
            last_synced_at: String::new(),
        };

        // Create the bucket (idempotent enough for a fresh name).
        let client = build_client(&settings.s3).expect("client");
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
