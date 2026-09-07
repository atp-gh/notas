//! Sync engine: S3-compatible object storage and WebDAV backends.
//!
//! The pure planning logic lives in `crate::sync` (see its module doc
//! for the object layout, conflict rules and tombstone semantics). This
//! module is the executor: each backend implements the `SyncStore` seam
//! — list the remote index, fetch/upload markdown bodies and sidecars —
//! and a shared `run_sync_with` turns a [`SyncAction`] plan into actual
//! store operations, then updates the local database. It runs on the DB
//! worker's tokio runtime, never on the UI thread.
//!
//! ## Encryption
//!
//! When enabled in Settings, end-to-end encryption is transparent to the
//! planner: `run_sync_with` resolves a [`Cipher`] up front (reading the
//! backend's `meta/.encryption-verifier` object, writing it on a
//! never-encrypted backend, and aborting on a wrong password before any
//! note traffic), then every store call seals objects on upload and opens
//! them on download. See [`crate::sync::crypto`] for the algorithm and blob
//! format.
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

use crate::core::repository::Repository;
use crate::core::sync::{
    LocalNote, RemoteEntry, Sidecar, SyncAction, SyncStats, content_hash, plan_sync,
};
use crate::sync::crypto::{self, Cipher, CryptoError, Verifier};
use crate::sync::error::SyncError;

use crate::application::config::{S3SyncSettings, SyncSettings, SyncType, WebDavSyncSettings};

/// The storage primitives the planner's actions map onto. Implemented by
/// every sync backend; the rest of [`run_sync_with`] is shared.
///
/// Encryption is transparent to the planner: a [`Cipher`] is passed into
/// every method when end-to-end encryption is enabled, and each backend
/// seals objects on upload and opens them on download. The verifier
/// methods read/write `meta/.encryption-verifier`, the object that carries
/// the key salt and a wrong-password check (see
/// [`crate::sync::crypto::Verifier`]).
trait SyncStore {
    /// Prepare the backend for note traffic (WebDAV creates its
    /// collections; a no-op for S3). Runs before the verifier is touched
    /// so a fresh backend can receive one.
    async fn ensure_ready(&self) -> Result<(), SyncError>;
    /// List the whole remote store, fetching every sidecar, and build the
    /// remote index the planner needs. Sidecars are decrypted with `cipher`
    /// when encryption is active; ones that fail to decrypt are logged and
    /// skipped (the planner re-uploads a matching local note).
    async fn list(
        &self,
        cipher: Option<&Cipher>,
    ) -> Result<HashMap<String, RemoteEntry>, SyncError>;
    /// Fetch the markdown body of a note, decrypted when a cipher is active.
    async fn get_md(&self, cipher: Option<&Cipher>, uuid: &str) -> Result<String, SyncError>;
    /// Upload a note's markdown body and sidecar (encrypted when a cipher
    /// is active).
    async fn put_note(&self, cipher: Option<&Cipher>, note: &LocalNote) -> Result<(), SyncError>;
    /// Upload a sidecar alone, used for tombstones (encrypted likewise).
    async fn put_sidecar(
        &self,
        cipher: Option<&Cipher>,
        sidecar: &Sidecar,
    ) -> Result<(), SyncError>;
    /// Read the encryption verifier object, or `None` when the backend has
    /// never been encrypted (or the object was deleted).
    async fn get_verifier(&self) -> Result<Option<Vec<u8>>, SyncError>;
    /// Write the encryption verifier object.
    async fn put_verifier(&self, bytes: &[u8]) -> Result<(), SyncError>;
}

/// Run one full sync against the configured backend, then return stats.
pub async fn run_sync(pool: &SqlitePool, settings: &SyncSettings) -> Result<SyncStats, SyncError> {
    match settings.kind {
        SyncType::S3 => {
            let store = S3Store::new(&settings.s3)?;
            run_sync_with(&Repository::new(pool.clone()), &store, settings).await
        }
        SyncType::WebDAV => {
            let store = WebDavStore::new(&settings.webdav)?;
            run_sync_with(&Repository::new(pool.clone()), &store, settings).await
        }
    }
}

/// Plan and execute one sync against any [`SyncStore`]: build both
/// indexes, run the pure planner, execute every action, and record the
/// run's timestamp.
async fn run_sync_with(
    repo: &Repository,
    store: &impl SyncStore,
    settings: &SyncSettings,
) -> Result<SyncStats, SyncError> {
    // 0. Backend readiness + encryption state. Resolving the cipher reads
    //    (or, on a never-encrypted backend, writes) the verifier object, so
    //    a wrong password aborts before any note traffic can clobber the
    //    remote copy.
    store.ensure_ready().await?;
    let cipher = resolve_cipher(store, settings).await?;

    // 1. Assign uuids to notes created since the last sync so every note
    //    has a stable cross-device identity.
    repo.ensure_note_uuids().await?;

    // 2. Remote index + local index + tombstones.
    let remote = store.list(cipher.as_ref()).await?;
    let local = repo.sync_local_index().await?;
    let tombstones = repo.list_tombstones().await?;

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
                store.put_note(cipher.as_ref(), &note).await?;
                stats.uploaded += 1;
            }
            SyncAction::Download { sidecar } => {
                let content = match store.get_md(cipher.as_ref(), &sidecar.uuid).await {
                    Ok(content) => content,
                    // Corrupt/foreign body (or, with encryption, a blob
                    // that fails to open): skip the note instead of
                    // aborting the whole sync; the next sync retries it.
                    Err(e) => {
                        eprintln!(
                            "notas: skipping note {} — cannot download it: {e}",
                            sidecar.uuid
                        );
                        continue;
                    }
                };
                repo.apply_remote_note(&sidecar, &content).await?;
                stats.downloaded += 1;
            }
            SyncAction::TrashLocal { uuid } => {
                repo.trash_note_by_uuid_no_bump(&uuid).await?;
                stats.trashed += 1;
            }
            SyncAction::ConflictCopy { sidecar } => {
                let content = match store.get_md(cipher.as_ref(), &sidecar.uuid).await {
                    Ok(content) => content,
                    // Same tolerance as downloads: a remote body that
                    // cannot be fetched/opened is skipped, keeping the
                    // local note and the rest of the sync intact.
                    Err(e) => {
                        eprintln!("notas: skipping conflict copy for {} — {e}", sidecar.uuid);
                        continue;
                    }
                };
                repo.create_conflict_copy(&sidecar, &content).await?;
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
                store.put_sidecar(cipher.as_ref(), &sidecar).await?;
                stats.uploaded += 1;
            }
        }
    }

    // 4. Timestamp of this run, in the device's local time (display only;
    //    note timestamps themselves stay UTC in the database).
    stats.last_synced_at = sqlx::query_scalar::<_, String>("SELECT datetime('now', 'localtime')")
        .fetch_one(repo.pool())
        .await?;

    Ok(stats)
}

/// Establish the encryption state for one sync run.
///
/// - **Encryption disabled**: refuse the sync when the backend is already
///   encrypted (a verifier exists) — otherwise the plaintext uploads would
///   silently replace the encrypted data. Plain backends proceed as before.
/// - **Encryption enabled, verifier present**: derive the key from the
///   password and verify it against the verifier's check value. A mismatch
///   aborts the whole sync: with a wrong password every sidecar would fail
///   to decrypt and the planner would re-upload local notes over the
///   encrypted originals, destroying them.
/// - **Encryption enabled, no verifier**: the backend has never been
///   encrypted (fresh bucket, or plaintext data being migrated). Generate a
///   fresh salt and persist the verifier so every device derives the same
///   key. The migration itself is handled by the planner: plaintext
///   sidecars fail to open and are skipped, so matching local notes
///   re-upload encrypted (overwrite-in-place).
async fn resolve_cipher(
    store: &impl SyncStore,
    settings: &SyncSettings,
) -> Result<Option<Cipher>, SyncError> {
    let enc = &settings.encryption;
    let verifier_bytes = store.get_verifier().await?;

    if !enc.enabled {
        if verifier_bytes.is_some() {
            return Err(SyncError::configuration(
                "this backend is encrypted — enable “Encrypt synced notes” in Settings and \
                 enter the encryption password",
            ));
        }
        return Ok(None);
    }

    let password = enc.password.trim();
    if password.len() < crypto::MIN_PASSWORD_LEN {
        return Err(SyncError::configuration(format!(
            "the encryption password must be at least {} characters",
            crypto::MIN_PASSWORD_LEN
        )));
    }

    match verifier_bytes {
        Some(bytes) => {
            let verifier: Verifier = serde_json::from_slice(&bytes).map_err(|e| {
                SyncError::configuration(format!(
                    "cannot read the encryption verifier on the backend: {e}"
                ))
            })?;
            let cipher = Cipher::derive(password, verifier.salt_bytes()?)?;
            if !cipher.verify(&verifier) {
                return Err(SyncError::configuration(
                    "the encryption password is wrong — enter the password that encrypted \
                     this backend",
                ));
            }
            Ok(Some(cipher))
        }
        None => {
            let cipher = Cipher::generate(password)?;
            let verifier = cipher.verifier()?;
            let bytes = serde_json::to_vec(&verifier)
                .map_err(|e| SyncError::configuration(format!("cannot build the verifier: {e}")))?;
            store.put_verifier(&bytes).await?;
            Ok(Some(cipher))
        }
    }
}

/// Seal a plaintext body for upload when encryption is active.
///
/// Returns the typed [`CryptoError`]; callers convert it to a message
/// through the `From<CryptoError> for String` impl on the seam (or log
/// it directly, since `Display` is user-facing).
fn encrypt_body(cipher: Option<&Cipher>, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    match cipher {
        Some(c) => c.encrypt(plaintext),
        None => Ok(plaintext.to_vec()),
    }
}

/// Open a downloaded object when encryption is active.
fn decrypt_body(cipher: Option<&Cipher>, blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    match cipher {
        Some(c) => c.decrypt(blob),
        None => Ok(blob.to_vec()),
    }
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
    fn new(settings: &S3SyncSettings) -> Result<Self, SyncError> {
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
    async fn ensure_ready(&self) -> Result<(), SyncError> {
        Ok(())
    }

    async fn list(
        &self,
        cipher: Option<&Cipher>,
    ) -> Result<HashMap<String, RemoteEntry>, SyncError> {
        list_remote(&self.client, &self.bucket, &self.prefix, cipher).await
    }

    async fn get_md(&self, cipher: Option<&Cipher>, uuid: &str) -> Result<String, SyncError> {
        get_md(&self.client, &self.bucket, &self.prefix, cipher, uuid).await
    }

    async fn put_note(&self, cipher: Option<&Cipher>, note: &LocalNote) -> Result<(), SyncError> {
        put_note(&self.client, &self.bucket, &self.prefix, cipher, note).await
    }

    async fn put_sidecar(
        &self,
        cipher: Option<&Cipher>,
        sidecar: &Sidecar,
    ) -> Result<(), SyncError> {
        put_sidecar(&self.client, &self.bucket, &self.prefix, cipher, sidecar).await
    }

    async fn get_verifier(&self) -> Result<Option<Vec<u8>>, SyncError> {
        get_verifier(&self.client, &self.bucket, &self.prefix).await
    }

    async fn put_verifier(&self, bytes: &[u8]) -> Result<(), SyncError> {
        put_verifier(&self.client, &self.bucket, &self.prefix, bytes).await
    }
}

/// Build the S3 client from the configured endpoint/region/credentials.
///
/// An empty endpoint means AWS S3 (virtual-hosted addressing on
/// `https://s3.<region>.amazonaws.com`); a custom endpoint (R2, B2,
/// MinIO, …) switches to path-style addressing, which those
/// S3-compatible servers expect.
fn build_client(settings: &S3SyncSettings) -> Result<S3Client, SyncError> {
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
        .map_err(|e| SyncError::transport(format!("{e:#}")))?,
    );
    S3Client::builder(endpoint)
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
        .region(region)
        .auth(auth)
        .addressing_style(addressing)
        .timeout(Duration::from_secs(30))
        .max_attempts(3)
        .build()
        .map_err(|e| SyncError::transport(format!("{e:#}")))
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

/// Key of the encryption verifier object. It lives inside `meta/` but has
/// no `.json` suffix, so the listing parsers ([`parse_key`] and its WebDAV
/// counterpart) never mistake it for a note sidecar.
fn verifier_key(prefix: &str) -> String {
    format!("{prefix}meta/.encryption-verifier")
}

/// List the whole prefix and fetch every sidecar, building the remote
/// index the planner needs. Sidecars are decrypted when `cipher` is set;
/// ones that fail to open are logged and skipped like unparseable ones.
async fn list_remote(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
    cipher: Option<&Cipher>,
) -> Result<HashMap<String, RemoteEntry>, SyncError> {
    let mut remote: HashMap<String, RemoteEntry> = HashMap::new();
    let mut pager = client
        .objects()
        .list_v2(bucket)
        .prefix(prefix)
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
        .pager();
    while let Some(page) = pager
        .next_page()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
    {
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
        let plain = match decrypt_body(cipher, &bytes) {
            Ok(plain) => plain,
            Err(e) => {
                eprintln!("notas: cannot decrypt sidecar for {uuid}: {e}");
                continue;
            }
        };
        match serde_json::from_slice::<Sidecar>(&plain) {
            Ok(sidecar) => match sidecar.validated() {
                Ok(valid) => {
                    remote.entry(uuid).or_default().sidecar = Some(valid.clone());
                }
                Err(e) => {
                    eprintln!("notas: ignoring invalid sidecar for {uuid}: {e}");
                }
            },
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
    cipher: Option<&Cipher>,
    note: &LocalNote,
) -> Result<(), SyncError> {
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
    let body = encrypt_body(cipher, note.content.as_bytes())?;
    client
        .objects()
        .put(bucket, md_key(prefix, &note.uuid))
        .content_type("text/markdown; charset=utf-8")
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
        .body_bytes(body)
        .send()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    put_sidecar(client, bucket, prefix, cipher, &sidecar).await
}

async fn put_sidecar(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
    cipher: Option<&Cipher>,
    sidecar: &Sidecar,
) -> Result<(), SyncError> {
    let json = serde_json::to_string(sidecar)
        .map_err(|e| SyncError::transport(format!("cannot encode the sidecar: {e}")))?;
    let body = encrypt_body(cipher, json.as_bytes())?;
    client
        .objects()
        .put(bucket, meta_key(prefix, &sidecar.uuid))
        .content_type("application/json")
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
        .body_bytes(body)
        .send()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    Ok(())
}

async fn get_md(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
    cipher: Option<&Cipher>,
    uuid: &str,
) -> Result<String, SyncError> {
    let output = client
        .objects()
        .get(bucket, md_key(prefix, uuid))
        .send()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    let bytes = output
        .bytes()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    let plain = decrypt_body(cipher, &bytes)?;
    String::from_utf8(plain)
        .map_err(|e| SyncError::transport(format!("note {uuid} is not valid UTF-8: {e}")))
}

/// Fetch the verifier object; a 404 (never encrypted) is `None`.
async fn get_verifier(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
) -> Result<Option<Vec<u8>>, SyncError> {
    let key = verifier_key(prefix);
    let output = match client.objects().get(bucket, &key).send().await {
        Ok(output) => output,
        // A missing object is the normal "never encrypted" answer.
        Err(e) if e.status().is_some_and(|s| s.as_u16() == 404) => return Ok(None),
        Err(e) => {
            return Err(SyncError::transport(format!(
                "cannot read the encryption verifier: {e:#}"
            )));
        }
    };
    let bytes = output
        .bytes()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    Ok(Some(bytes.to_vec()))
}

/// Write the verifier object.
async fn put_verifier(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
    bytes: &[u8],
) -> Result<(), SyncError> {
    client
        .objects()
        .put(bucket, verifier_key(prefix))
        .content_type("application/json")
        .map_err(|e| SyncError::transport(format!("{e:#}")))?
        .body_bytes(bytes.to_vec())
        .send()
        .await
        .map_err(|e| SyncError::transport(format!("{e:#}")))?;
    Ok(())
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
    fn new(settings: &WebDavSyncSettings) -> Result<Self, SyncError> {
        let url = webdav_base_url(settings)?;
        let agent = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(settings.insecure_tls)
            .build()
            .map_err(|e| SyncError::transport(format!("cannot build the HTTP client: {e}")))?;
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
            .map_err(|e| SyncError::transport(format!("cannot build the WebDAV client: {e}")))?;
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
    async fn ensure_collections(&self) -> Result<(), SyncError> {
        // The URL itself is expected to exist; only an explicit directory
        // is created. Without one, the notes go straight into the URL.
        if !self.directory.is_empty() {
            self.ensure_collection(&self.directory).await?;
        }
        self.ensure_collection(&self.notes_collection()).await?;
        self.ensure_collection(&self.meta_collection()).await
    }

    async fn ensure_collection(&self, path: &str) -> Result<(), SyncError> {
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
    async fn list_collection(
        &self,
        collection: &str,
        suffix: &str,
    ) -> Result<Vec<String>, SyncError> {
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
    /// between listing and fetching, or the file is foreign), or when the
    /// ciphertext fails to decrypt or parse.
    async fn get_sidecar(
        &self,
        cipher: Option<&Cipher>,
        uuid: &str,
    ) -> Result<Option<Sidecar>, SyncError> {
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
            .map_err(|e| SyncError::transport(format!("cannot read sidecar for {uuid}: {e}")))?;
        let plain = match decrypt_body(cipher, &bytes) {
            Ok(plain) => plain,
            Err(e) => {
                eprintln!("notas: cannot decrypt sidecar for {uuid}: {e}");
                return Ok(None);
            }
        };
        match serde_json::from_slice::<Sidecar>(&plain) {
            Ok(sidecar) => Ok(sidecar.validated().cloned().ok()),
            Err(e) => {
                eprintln!("notas: ignoring unparseable sidecar for {uuid}: {e}");
                Ok(None)
            }
        }
    }

    /// PUT one file with an explicit content type. The body is bytes so
    /// encrypted objects (binary ciphertext) need no base64 round-trip.
    async fn put(&self, path: &str, content_type: &str, body: Vec<u8>) -> Result<(), SyncError> {
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
            .map_err(|e| SyncError::transport(format!("cannot upload: {e}")))?;
        let code = response.status().as_u16();
        if response.status().is_success() {
            Ok(())
        } else {
            Err(webdav_status_error("upload", code))
        }
    }

    /// Path of the encryption verifier object, inside the meta collection
    /// but without a `.json` suffix so the collection listing never
    /// mistakes it for a note sidecar.
    fn verifier_path(&self) -> String {
        format!("{}/.encryption-verifier", self.meta_collection())
    }

    /// GET the verifier object; `None` when the server says 404 (the
    /// backend has never been encrypted).
    async fn get_verifier(&self) -> Result<Option<Vec<u8>>, SyncError> {
        let response = self
            .client
            .get_raw(&self.verifier_path())
            .await
            .map_err(webdav_error)?;
        let code = response.status().as_u16();
        if code == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(webdav_status_error("read encryption verifier", code));
        }
        let bytes = response.bytes().await.map_err(|e| {
            SyncError::transport(format!("cannot read the encryption verifier: {e}"))
        })?;
        Ok(Some(bytes.to_vec()))
    }

    /// PUT the verifier object.
    async fn put_verifier(&self, bytes: &[u8]) -> Result<(), SyncError> {
        self.put(&self.verifier_path(), "application/json", bytes.to_vec())
            .await
    }
}

impl SyncStore for WebDavStore {
    async fn ensure_ready(&self) -> Result<(), SyncError> {
        // Runs before the verifier is touched so a fresh backend can
        // receive the verifier object.
        self.ensure_collections().await
    }

    async fn list(
        &self,
        cipher: Option<&Cipher>,
    ) -> Result<HashMap<String, RemoteEntry>, SyncError> {
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
            match self.get_sidecar(cipher, &uuid).await {
                Ok(Some(sidecar)) => {
                    remote.entry(uuid).or_default().sidecar = Some(sidecar);
                }
                Ok(None) => {
                    // Vanished, undecryptable or unparseable: leave the
                    // entry without a sidecar; the planner re-uploads if a
                    // local note matches, and ignores it otherwise.
                }
                Err(e) => eprintln!("notas: cannot read sidecar for {uuid}: {e}"),
            }
        }
        Ok(remote)
    }

    async fn get_md(&self, cipher: Option<&Cipher>, uuid: &str) -> Result<String, SyncError> {
        let path = format!("{}/{uuid}.md", self.notes_collection());
        let response = self.client.get_raw(&path).await.map_err(webdav_error)?;
        let code = response.status().as_u16();
        if !response.status().is_success() {
            return Err(webdav_status_error(&format!("read note {uuid}"), code));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| SyncError::transport(format!("cannot read note {uuid}: {e}")))?;
        let plain = decrypt_body(cipher, &bytes)?;
        String::from_utf8(plain)
            .map_err(|e| SyncError::transport(format!("note {uuid} is not valid UTF-8: {e}")))
    }

    async fn put_note(&self, cipher: Option<&Cipher>, note: &LocalNote) -> Result<(), SyncError> {
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
        let body = encrypt_body(cipher, note.content.as_bytes())?;
        self.put(&md_path, "text/markdown; charset=utf-8", body)
            .await?;
        self.put_sidecar(cipher, &sidecar).await
    }

    async fn put_sidecar(
        &self,
        cipher: Option<&Cipher>,
        sidecar: &Sidecar,
    ) -> Result<(), SyncError> {
        let path = format!("{}/{}.json", self.meta_collection(), sidecar.uuid);
        let json = serde_json::to_string(sidecar)
            .map_err(|e| SyncError::transport(format!("cannot encode the sidecar: {e}")))?;
        let body = encrypt_body(cipher, json.as_bytes())?;
        self.put(&path, "application/json", body).await
    }

    async fn get_verifier(&self) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_verifier().await
    }

    async fn put_verifier(&self, bytes: &[u8]) -> Result<(), SyncError> {
        self.put_verifier(bytes).await
    }
}

/// Resolve the configured URL into the base the WebDAV client talks to.
///
/// A missing scheme defaults to `https://`; plain `http://` is only
/// accepted when the user opted into insecure TLS.
fn webdav_base_url(settings: &WebDavSyncSettings) -> Result<String, SyncError> {
    let raw = settings.url.trim();
    if raw.is_empty() {
        return Err(SyncError::configuration(
            "WebDAV URL is empty — set the server URL in Settings",
        ));
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let parsed = reqwest::Url::parse(&with_scheme).map_err(|e| {
        SyncError::configuration(format!("WebDAV URL \"{raw}\" is not a valid URL: {e}"))
    })?;
    match parsed.scheme() {
        "https" => {}
        "http" if settings.insecure_tls => {}
        "http" => {
            return Err(SyncError::configuration(
                "WebDAV URL uses plain http — enable “allow insecure TLS” to accept it",
            ));
        }
        other => {
            return Err(SyncError::configuration(format!(
                "WebDAV URL scheme \"{other}\" is not supported — use https"
            )));
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

/// Map a `reqwest_dav` error to a typed sync error, extracting the
/// HTTP status where the crate wrapped it.
fn webdav_error(err: reqwest_dav::Error) -> SyncError {
    use reqwest_dav::DecodeError;
    match &err {
        reqwest_dav::Error::Decode(DecodeError::StatusMismatched(status)) => {
            webdav_status_error("request", status.response_code)
        }
        reqwest_dav::Error::Decode(DecodeError::Server(server)) => {
            webdav_status_error("request", server.response_code)
        }
        reqwest_dav::Error::Reqwest(e) => {
            SyncError::transport(format!("WebDAV network error: {e}"))
        }
        other => SyncError::transport(format!("WebDAV error: {other}")),
    }
}

/// Friendly typed error for a failed WebDAV operation with its status.
fn webdav_status_error(operation: &str, code: u16) -> SyncError {
    let message = match code {
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
    };
    SyncError::transport(message)
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
        assert!(err.to_string().contains("insecure"), "{err}");
        plain.insecure_tls = true;
        assert_eq!(
            webdav_base_url(&plain).unwrap(),
            "http://192.168.1.10/webdav"
        );

        let err = webdav_base_url(&webdav_settings("ftp://example.com")).unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");

        let mut empty = webdav_settings("");
        empty.url.clear();
        assert!(
            webdav_base_url(&empty)
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
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

    use crate::application::config::EncryptionSettings;
    use crate::storage::{db, repo};

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
        repo::update_note(&pool, note.id.0, "Hello", "body one")
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
            encryption: EncryptionSettings::default(),
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
            encryption: EncryptionSettings::default(),
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
        repo::update_note(&pool, note.id.0, "Old", "old body")
            .await
            .unwrap();
        repo::ensure_note_uuids(&pool).await.unwrap();
        sqlx::query("UPDATE notes SET uuid = ?1, updated_at = '2000-01-01 00:00:00' WHERE id = ?2")
            .bind(&uuid)
            .bind(note.id.0)
            .execute(&pool)
            .await
            .unwrap();

        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            encryption: EncryptionSettings::default(),
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
            encryption: EncryptionSettings::default(),
            last_synced_at: String::new(),
        };
        let err = run_sync(&pool, &settings).await.unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
        assert!(err.to_string().contains("app password"), "{err}");
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Encryption password shared by the encrypted-backend tests.
    const ENC_PASSWORD: &str = "correct horse battery staple";

    /// SyncSettings with WebDAV + encryption enabled for `password`.
    fn encrypted_settings(url: &str, password: &str) -> SyncSettings {
        SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(url),
            encryption: EncryptionSettings {
                enabled: true,
                password: password.into(),
            },
            last_synced_at: String::new(),
        }
    }

    /// A deterministic cipher for one test (fixed salt) so the fixture
    /// blobs and the client under test derive the same key.
    fn test_cipher(password: &str) -> Cipher {
        Cipher::derive(password, [3u8; 16]).expect("derive")
    }

    /// Seal a complete remote note under `cipher` and return the three
    /// fixture bodies: verifier JSON, encrypted sidecar, encrypted body.
    fn sealed_remote(
        cipher: &Cipher,
        uuid: &str,
        title: &str,
        body: &str,
    ) -> (String, Vec<u8>, Vec<u8>) {
        let sidecar = Sidecar {
            uuid: uuid.into(),
            title: title.into(),
            notebook: None,
            tags: vec!["work".into()],
            trashed: false,
            deleted: false,
            updated_at: "2026-01-02 03:04:05".into(),
            content_hash: content_hash(body),
        };
        let verifier = serde_json::to_string(&cipher.verifier().expect("verifier")).unwrap();
        (
            verifier,
            cipher
                .encrypt(&serde_json::to_string(&sidecar).unwrap().into_bytes())
                .expect("seal sidecar"),
            cipher.encrypt(body.as_bytes()).expect("seal body"),
        )
    }

    #[tokio::test]
    async fn webdav_executor_encrypts_uploaded_notes() {
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

        let dir = temp_db_dir("enc-upload");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let note = repo::create_note(&pool, None, "Hello").await.unwrap();
        repo::update_note(&pool, note.id.0, "Hello", "body one")
            .await
            .unwrap();
        repo::ensure_note_uuids(&pool).await.unwrap();
        let uuid = repo::sync_local_index(&pool).await.unwrap()[0].uuid.clone();

        let verifier_path = "/notas/meta/.encryption-verifier";
        // A fresh backend: no verifier yet, so the sync establishes one.
        Mock::given(method("GET"))
            .and(path(verifier_path))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(verifier_path))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let md_path = format!("/notas/notes/{uuid}.md");
        let meta_path = format!("/notas/meta/{uuid}.json");
        Mock::given(method("PUT"))
            .and(path(md_path.clone()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        // The uploaded body must not contain the plaintext: `body_string_contains`
        // never matches binary ciphertext (invalid UTF-8), so any hit means
        // the note went up in clear.
        Mock::given(method("PUT"))
            .and(path(md_path))
            .and(wiremock::matchers::body_string_contains("body one"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(meta_path))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let settings = encrypted_settings(&server.uri(), ENC_PASSWORD);
        let stats = run_sync(&pool, &settings).await.expect("encrypted sync");
        assert_eq!(stats.uploaded, 1);
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_downloads_and_decrypts_remote_notes() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let (verifier_json, sidecar_blob, md_blob) =
            sealed_remote(&test_cipher(ENC_PASSWORD), &uuid, "Remote", "remote body");
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
        Mock::given(method("GET"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_string(verifier_json))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{uuid}.json")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(sidecar_blob))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/notes/{uuid}.md")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(md_blob))
            .mount(&server)
            .await;

        let dir = temp_db_dir("enc-download");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let settings = encrypted_settings(&server.uri(), ENC_PASSWORD);
        let stats = run_sync(&pool, &settings).await.expect("encrypted sync");
        assert_eq!(stats.downloaded, 1);

        let notes = repo::list_all_notes(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Remote");
        assert_eq!(notes[0].content, "remote body");
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_skips_undecryptable_body_and_syncs_the_rest() {
        // Two remote notes whose sidecars both decrypt fine (so the planner
        // issues a Download for each), but the second note's body is
        // tampered ciphertext: its auth tag fails, so the executor must
        // skip just that note and keep the rest of the sync running.
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let good_uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let bad_uuid = "01234567-89ab-4cde-8f01-23456789abce".to_string();
        let (verifier_json, good_sidecar, good_md) =
            sealed_remote(&test_cipher(ENC_PASSWORD), &good_uuid, "Good", "good body");
        let (_, bad_sidecar, mut bad_md) =
            sealed_remote(&test_cipher(ENC_PASSWORD), &bad_uuid, "Bad", "bad body");
        // Bit rot in the ciphertext: flip one byte so the authentication
        // tag (not the header) fails.
        let last = bad_md.len() - 1;
        bad_md[last] ^= 0xff;

        let good_md_href = format!("/notas/notes/{good_uuid}.md");
        let bad_md_href = format!("/notas/notes/{bad_uuid}.md");
        let good_meta_href = format!("/notas/meta/{good_uuid}.json");
        let bad_meta_href = format!("/notas/meta/{bad_uuid}.json");

        Mock::given(method("PROPFIND"))
            .and(path("/notas/notes"))
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus(&[good_md_href, bad_md_href])),
            )
            .mount(&server)
            .await;
        Mock::given(method("PROPFIND"))
            .and(path("/notas/meta"))
            .respond_with(
                ResponseTemplate::new(207)
                    .set_body_string(multistatus(&[good_meta_href, bad_meta_href])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_string(verifier_json))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{good_uuid}.json")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(good_sidecar))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{bad_uuid}.json")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bad_sidecar))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/notes/{good_uuid}.md")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(good_md))
            .expect(1)
            .mount(&server)
            .await;
        // The corrupt body is fetched once (the skip happens at decrypt
        // time, not before the download), then the note is dropped.
        Mock::given(method("GET"))
            .and(path(format!("/notas/notes/{bad_uuid}.md")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bad_md))
            .expect(1)
            .mount(&server)
            .await;

        let dir = temp_db_dir("enc-skip-body");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let settings = encrypted_settings(&server.uri(), ENC_PASSWORD);
        let stats = run_sync(&pool, &settings)
            .await
            .expect("sync must survive a corrupt note");

        // The healthy note downloaded, the corrupt one was skipped; the
        // sync itself did not fail and nothing else was recorded.
        assert_eq!(stats.downloaded, 1);
        assert_eq!(stats.conflicts, 0);
        let notes = repo::list_all_notes(&pool).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Good");
        assert_eq!(notes[0].content, "good body");
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_aborts_on_wrong_encryption_password() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let (verifier_json, _, _) =
            sealed_remote(&test_cipher(ENC_PASSWORD), &uuid, "Remote", "body");

        Mock::given(method("GET"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_string(verifier_json))
            .mount(&server)
            .await;
        // The wrong password must abort before any listing or note traffic:
        // with it, every sidecar would fail to decrypt and the planner
        // would re-upload local notes over the encrypted originals.
        Mock::given(method("PROPFIND"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let dir = temp_db_dir("enc-wrong-pw");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let settings = encrypted_settings(&server.uri(), "not the right password");
        let err = run_sync(&pool, &settings).await.unwrap_err();
        assert!(err.to_string().contains("password"), "{err}");
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_refuses_plaintext_sync_of_an_encrypted_backend() {
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let (verifier_json, _, _) =
            sealed_remote(&test_cipher(ENC_PASSWORD), &uuid, "Remote", "body");

        Mock::given(method("GET"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_string(verifier_json))
            .mount(&server)
            .await;
        Mock::given(method("PROPFIND"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let dir = temp_db_dir("enc-refuse");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        // Encryption disabled: syncing would upload plaintext over the
        // encrypted remote data, so the sync must refuse instead.
        let settings = SyncSettings {
            kind: SyncType::WebDAV,
            s3: S3SyncSettings::default(),
            webdav: webdav_settings(&server.uri()),
            encryption: EncryptionSettings::default(),
            last_synced_at: String::new(),
        };
        let err = run_sync(&pool, &settings).await.unwrap_err();
        assert!(err.to_string().contains("encrypted"), "{err}");
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn webdav_executor_migrates_plaintext_remote_when_encryption_is_enabled() {
        // Q3(a): enabling encryption on a backend that already holds
        // plaintext notes. The plaintext sidecar fails to open (missing
        // magic) and is skipped, so the matching local note re-uploads
        // encrypted over the same key (overwrite-in-place).
        let server = MockServer::start().await;
        accept_mkcol(&server).await;
        let uuid = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let md_href = format!("/notas/notes/{uuid}.md");
        let meta_href = format!("/notas/meta/{uuid}.json");
        let plain_sidecar = serde_json::to_string(&Sidecar {
            uuid: uuid.clone(),
            title: "Old".into(),
            notebook: None,
            tags: Vec::new(),
            trashed: false,
            deleted: false,
            updated_at: "2000-01-01 00:00:00".into(),
            content_hash: content_hash("old body"),
        })
        .unwrap();

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
        Mock::given(method("GET"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/notas/meta/.encryption-verifier"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/notas/meta/{uuid}.json")))
            .respond_with(ResponseTemplate::new(200).set_body_string(plain_sidecar))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/notas/notes/{uuid}.md")))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/notas/meta/{uuid}.json")))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let dir = temp_db_dir("enc-migrate");
        let pool = db::connect(dir.join("a.db")).await.unwrap();
        let note = repo::create_note(&pool, None, "Old").await.unwrap();
        repo::update_note(&pool, note.id.0, "Old", "old body")
            .await
            .unwrap();
        repo::ensure_note_uuids(&pool).await.unwrap();
        sqlx::query("UPDATE notes SET uuid = ?1 WHERE id = ?2")
            .bind(&uuid)
            .bind(note.id.0)
            .execute(&pool)
            .await
            .unwrap();

        let settings = encrypted_settings(&server.uri(), ENC_PASSWORD);
        let stats = run_sync(&pool, &settings).await.expect("migrating sync");
        assert_eq!(stats.uploaded, 1);
        server.verify().await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ------------------------------------------------- S3 two-device e2e (manual)

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::application::config::EncryptionSettings;
    use crate::storage::{db, repo};

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
            encryption: EncryptionSettings::default(),
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
        let n1 = repo::create_note(&pool_a, Some(nb.id.0), "Hello")
            .await
            .unwrap();
        repo::update_note(&pool_a, n1.id.0, "Hello", "body one")
            .await
            .unwrap();
        let n2 = repo::create_note(&pool_a, None, "Second").await.unwrap();
        repo::set_note_tags(&pool_a, n2.id.0, &["meta".to_string()])
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
        let tags_b = repo::get_note_tags(&pool_b, hello_b.id.0).await.unwrap();
        assert!(tags_b.is_empty());
        let second_b = notes_b
            .iter()
            .find(|n| n.title == "Second")
            .expect("Second on B");
        let tags_b = repo::get_note_tags(&pool_b, second_b.id.0).await.unwrap();
        assert_eq!(
            tags_b.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["meta"]
        );

        // --- device B edits and creates; A picks it up --------------------
        tokio::time::sleep(Duration::from_secs(1)).await;
        repo::update_note(&pool_b, hello_b.id.0, "Hello", "edited on B")
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
        repo::delete_note_forever(&pool_a, n2.id.0).await.unwrap();
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

    /// The user-facing encryption scenario, end to end: device A encrypts
    /// and uploads, device B downloads and decrypts with the same password.
    /// Manual, needs a running S3-compatible server (see the doc comment on
    /// [`sync_e2e_minio`] for how to start MinIO).
    #[tokio::test]
    #[ignore = "requires a running S3-compatible server (see doc comment)"]
    async fn sync_e2e_minio_encrypted() {
        const PASSWORD: &str = "correct horse battery staple";
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
        let bucket = format!("notas-e2e-enc-{unique}");

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
            s3: s3.clone(),
            webdav: WebDavSyncSettings::default(),
            encryption: EncryptionSettings {
                enabled: true,
                password: PASSWORD.into(),
            },
            last_synced_at: String::new(),
        };

        let client = build_client(&s3).expect("client");
        client
            .buckets()
            .create(&bucket)
            .send()
            .await
            .expect("create bucket");

        let dir = std::env::temp_dir().join(format!("notas-sync-e2e-enc-{unique}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pool_a = db::connect(dir.join("a.db")).await.expect("db a");
        let pool_b = db::connect(dir.join("b.db")).await.expect("db b");

        // --- device A: create + encrypt + upload -------------------------
        let n1 = repo::create_note(&pool_a, None, "Secret").await.unwrap();
        repo::update_note(&pool_a, n1.id.0, "Secret", "classified body")
            .await
            .unwrap();
        let stats = run_sync(&pool_a, &settings).await.expect("A sync");
        assert_eq!(stats.uploaded, 1, "A uploads the encrypted note");

        // The object on the backend must not contain the plaintext.
        let uuid = repo::sync_local_index(&pool_a).await.unwrap()[0]
            .uuid
            .clone();
        let obj = client
            .objects()
            .get(&bucket, format!("notas/notes/{uuid}.md"))
            .send()
            .await
            .expect("get object");
        let bytes = obj.bytes().await.expect("object bytes");
        assert!(
            !bytes
                .windows("classified body".len())
                .any(|w| w == b"classified body"),
            "remote object must not contain the plaintext body"
        );

        // --- device B: download + decrypt with the same password ---------
        let stats = run_sync(&pool_b, &settings).await.expect("B sync");
        assert_eq!(stats.downloaded, 1, "B downloads and decrypts the note");
        let notes_b = repo::list_all_notes(&pool_b).await.unwrap();
        assert_eq!(notes_b.len(), 1);
        assert_eq!(notes_b[0].title, "Secret");
        assert_eq!(notes_b[0].content, "classified body");

        // --- a second sync is a no-op -------------------------------------
        let stats = run_sync(&pool_a, &settings).await.expect("A sync #2");
        assert_eq!(stats.uploaded, 0);
        assert_eq!(stats.downloaded, 0);

        pool_a.close().await;
        pool_b.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
