# Sync

Sync is **manual**: ☰ menu → “Sync now…” pushes local changes up and pulls remote changes down in one pass. There is no background auto-sync. The status bar shows the state at a glance — “Syncing…” while a sync runs, and the time of the last successful sync afterwards.

Each note is identified across devices by a stable UUID (assigned on first sync; the SQLite `notes.id` never leaves the machine). Under the configured prefix / directory, every note is a pair of objects — the layout is identical for both backends:

```
notes/<uuid>.md        — the Markdown body
meta/<uuid>.json       — sidecar: title, notebook path, tags, trash state, updated_at, content hash
meta/.encryption-verifier — plaintext metadata about the encryption (salt + password check), only present when encryption is enabled
```

## Backends

**S3-compatible object storage** — AWS S3, Cloudflare R2, Backblaze B2, MinIO, ... Configure the endpoint (empty = AWS), region, bucket, key prefix (default `notas/`), and an access key/secret. The bucket itself must exist already.

**WebDAV** — Nextcloud, ownCloud, and any RFC-4918 server. Configure:

- _Server URL_ — a DAV collection you can write to. For Nextcloud that is `https://<host>/remote.php/dav/files/<user>`.
- _Directory_ (default `notas`) — the folder under that URL where the notes live; it is created automatically on the first sync. Empty the field to sync straight into the URL itself.
- _Username_ and _password_ (Basic auth). For Nextcloud, create an **app password** in your account settings and use that — it can be revoked without touching your main password.
- _Allow insecure TLS_ (off by default) — accepts self-signed / invalid certificates and plain `http://` URLs. Only enable this for a server you trust on a network you trust: with it on, credentials can be read in transit and the server's identity is not verified.

The settings dialog shows the fields of the selected backend; both backends' configuration is kept, so switching never loses what you entered.

## Encryption

With **Settings → Sync → Encryption → “Encrypt synced notes”** on, every uploaded object (bodies, sidecars, tombstones) is sealed with **XChaCha20-Poly1305** under a key derived from your password via **Argon2id**. The backend only ever stores ciphertext plus random UUIDs.

- Enter the **same password on every device** that syncs: each device derives the same key from the same password and salt, so notes encrypted on one device decrypt on another. There is **no password recovery** — a lost password means the backend data is unreadable.
- Enabling encryption **re-uploads everything**: the first sync after enabling writes every local note encrypted over its plaintext copy on the backend (and writes the verifier object). Enable it on a device that holds the notes you want to keep.
- A wrong password is caught **before any note traffic**: the sync reads the verifier object first, derives the key, and aborts with a clear error on mismatch, instead of overwriting encrypted data with garbage.
- Disabling encryption asks for the current password, then re-uploads everything in plaintext on the next sync. A device that syncs with encryption off against an encrypted backend is refused.
- The password is stored **in plaintext in the settings file**, like the sync credentials (Joplin-style): it protects against _backend_ compromise, not against theft of this machine.
- The Argon2id salt is not secret; it travels in the verifier object and in every blob header, so deleting the verifier object does not lose data (the salt is recovered from the objects themselves).
- S3 buckets with **object versioning** enabled may retain the old plaintext versions after re-encryption — purge old versions if the backend must not see plaintext at all.

## Conflict and deletion rules

- **Last-write-wins** on `updated_at` (UTC, second resolution).
- Same-second _content_ collision: both texts are kept — the local note stays, and the remote version is imported as a new “`<title> (conflict copy)`” note.
- Same-second metadata-only difference (title, notebook, tags, trash): the deterministically smaller tuple wins on every device, so state converges instead of oscillating.
- **Trash propagates** both ways (it's just a state in the sidecar).
- **“Delete forever” does not propagate as a deletion**: it uploads a tombstone sidecar, and other devices move their copy to the trash instead of deleting it — a permanent delete on one device never destroys the note elsewhere. Restoring a note on one device resurrects it everywhere; deleting a note on _all_ devices requires deleting each copy.

## Security

The access key / password are stored **in plaintext** in `settings.json` (like Joplin), entered in the UI. Use scoped credentials: a dedicated S3 key restricted to the sync bucket only, or a Nextcloud app password — so a leaked settings file only exposes that one bucket/account and can be revoked independently.
