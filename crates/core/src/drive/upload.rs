//! Google Drive resumable upload protocol (chunked PUTs to a session URI).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const RESUMABLE_INIT: &str =
    "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable";
/// Chunk size; must be a multiple of 256 KiB for all but the final chunk.
const CHUNK: u64 = 16 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 4;

#[derive(Deserialize)]
struct IdResp {
    id: String,
}

/// Upload `path` into `parent_id` as `name`, returning the new Drive file id.
/// `on_progress(sent, total)` is invoked after each chunk is acknowledged.
pub fn resumable_upload(
    client: &reqwest::blocking::Client,
    token: &str,
    path: &Path,
    name: &str,
    parent_id: &str,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<String> {
    let total = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len();

    let session = init_session(client, token, name, parent_id, total)?;

    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = vec![0u8; CHUNK as usize];
    let mut offset = 0u64;

    while offset < total {
        let want = std::cmp::min(CHUNK, total - offset) as usize;
        file.seek(SeekFrom::Start(offset))?;
        read_full(&mut file, &mut buf[..want])?;
        let range = content_range(offset, offset + want as u64 - 1, total);

        let maybe_id = put_chunk_with_retry(client, &session, &range, &buf[..want])?;
        offset += want as u64;
        on_progress(offset, total);
        if let Some(id) = maybe_id {
            return Ok(id); // final chunk returned the file resource
        }
    }
    bail!("upload finished sending bytes but Drive never returned a file id");
}

/// Start a resumable session and return its upload URI.
fn init_session(
    client: &reqwest::blocking::Client,
    token: &str,
    name: &str,
    parent_id: &str,
    total: u64,
) -> Result<String> {
    let body = serde_json::json!({ "name": name, "parents": [parent_id] });
    let resp = client
        .post(RESUMABLE_INIT)
        .bearer_auth(token)
        .header("X-Upload-Content-Type", "video/mp4")
        .header("X-Upload-Content-Length", total.to_string())
        .json(&body)
        .send()
        .context("initiating resumable session")?;
    if !resp.status().is_success() {
        bail!(
            "resumable init failed ({}): {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }
    resp.headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .context("resumable session response had no Location header")
}

/// PUT one chunk, retrying transient failures. Returns `Some(id)` when this was
/// the final chunk (Drive replies 200/201 with the file resource).
fn put_chunk_with_retry(
    client: &reqwest::blocking::Client,
    session: &str,
    range: &str,
    chunk: &[u8],
) -> Result<Option<String>> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        // Re-sending the same Content-Range is idempotent, so retrying a failed
        // chunk without re-querying the offset is safe.
        let result = client
            .put(session)
            .header("Content-Range", range)
            .body(chunk.to_vec())
            .send();

        match result {
            Ok(resp) => {
                let code = resp.status().as_u16();
                match code {
                    308 => return Ok(None), // resume incomplete -> next chunk
                    200 | 201 => {
                        let text = resp.text().unwrap_or_default();
                        let parsed: IdResp = serde_json::from_str(&text)
                            .context("parsing completed-upload response")?;
                        return Ok(Some(parsed.id));
                    }
                    429 | 500 | 502 | 503 | 504 if attempt < MAX_ATTEMPTS => {
                        sleep(backoff(attempt));
                    }
                    _ => bail!(
                        "chunk PUT failed ({code}): {}",
                        resp.text().unwrap_or_default()
                    ),
                }
            }
            Err(e) if attempt < MAX_ATTEMPTS => {
                tracing::warn!("chunk PUT error (attempt {attempt}): {e}");
                sleep(backoff(attempt));
            }
            Err(e) => return Err(e).context("chunk PUT"),
        }
    }
}

/// Exponential backoff: 1s, 2s, 4s, …
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << (attempt - 1).min(5))
}

/// `bytes start-end/total`, e.g. `bytes 0-16777215/52428800`.
fn content_range(start: u64, end: u64, total: u64) -> String {
    format!("bytes {start}-{end}/{total}")
}

/// Read exactly `buf.len()` bytes or error.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            bail!("unexpected EOF (read {filled} of {} bytes)", buf.len());
        }
        filled += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn content_range_formats_bytes() {
        assert_eq!(
            content_range(0, 16_777_215, 52_428_800),
            "bytes 0-16777215/52428800"
        );
        assert_eq!(
            content_range(16_777_216, 20_000_000, 20_000_001),
            "bytes 16777216-20000000/20000001"
        );
    }

    #[test]
    fn read_full_fills_buffer() {
        let mut cur = Cursor::new(vec![1u8, 2, 3, 4, 5]);
        let mut buf = [0u8; 5];
        read_full(&mut cur, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn read_full_errors_on_short_read() {
        let mut cur = Cursor::new(vec![1u8, 2, 3]);
        let mut buf = [0u8; 5];
        assert!(read_full(&mut cur, &mut buf).is_err());
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(10), Duration::from_secs(32)); // capped at 1<<5
    }
}
