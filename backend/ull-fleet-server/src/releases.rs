//! Releases: signed application images and deploying them to devices.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use ull_fleet_sign::appdesc::AppDesc;

use crate::error::{AppError, AppResult};
use crate::fleet::{self, UpdateState};

#[derive(Debug, Clone)]
pub struct Release {
    pub id: i64,
    pub project: String,
    pub version: String,
    pub secure_version: i64,
    pub size: i64,
    pub sha256: String,
    pub signature: String,
    pub file_name: String,
    pub build_date: String,
    pub notes: String,
    pub created_at: i64,
}

const COLUMNS: &str = "id, project, version, secure_version, size, sha256, signature, file_name,
    build_date, notes, created_at";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Release> {
    Ok(Release {
        id: row.get(0)?,
        project: row.get(1)?,
        version: row.get(2)?,
        secure_version: row.get(3)?,
        size: row.get(4)?,
        sha256: row.get(5)?,
        signature: row.get(6)?,
        file_name: row.get(7)?,
        build_date: row.get(8)?,
        notes: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Option<Release>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM releases WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?)
}

/// All releases, newest build first within each project.
pub fn list(conn: &Connection) -> AppResult<Vec<Release>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM releases ORDER BY project, secure_version DESC, id DESC"
    ))?;
    let releases = stmt.query_map([], from_row)?.collect::<Result<_, _>>()?;
    Ok(releases)
}

/// An upload that passed every check and is ready to store.
pub struct Validated {
    pub desc: AppDesc,
    pub sha256: [u8; 32],
    pub signature: [u8; 64],
}

/// Checks an uploaded image: ESP app image with a descriptor, a signature,
/// and (when the server has a key) a signature by that key.
pub fn validate(
    image: &[u8],
    signature_hex: &str,
    public_key: Option<&[u8; 32]>,
) -> AppResult<Validated> {
    if image.is_empty() {
        return Err(AppError::bad_request("the image is empty"));
    }
    let desc = AppDesc::parse(image).map_err(|err| AppError::bad_request(err.to_string()))?;
    if desc.project_name.is_empty() {
        return Err(AppError::bad_request(
            "the app descriptor has no project name",
        ));
    }
    let signature_hex = signature_hex.trim();
    if signature_hex.is_empty() {
        return Err(AppError::bad_request(
            "a signature is required; create one with `ull-fleet-sign sign <key> <image>`",
        ));
    }
    let signature: [u8; 64] = ull_fleet_sign::from_hex(signature_hex)
        .map_err(|err| AppError::bad_request(format!("invalid signature: {err}")))?;
    if let Some(key) = public_key {
        ull_fleet_sign::verify_image(key, image, &signature).map_err(|_| {
            AppError::bad_request("the signature does not match this image and the server's key")
        })?;
    }
    Ok(Validated {
        desc,
        sha256: ull_fleet_sign::sha256(image),
        signature,
    })
}

/// Writes the image under its content hash (temp file + rename, so a crash
/// never leaves a partial image) and records the release.
pub async fn store(
    db: &crate::db::Db,
    releases_dir: &Path,
    image: Vec<u8>,
    validated: Validated,
    notes: String,
    now: i64,
) -> AppResult<Release> {
    let Validated {
        desc,
        sha256,
        signature,
    } = validated;
    let project = desc.project_name.clone();
    let version = desc.version.clone();
    let exists = db
        .call({
            let (project, version) = (project.clone(), version.clone());
            move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM releases WHERE project = ?1 AND version = ?2",
                        [&project, &version],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            }
        })
        .await?;
    if exists {
        return Err(AppError::Conflict(format!(
            "{project} {version} is already uploaded; rebuild to get a new version"
        )));
    }

    let sha_hex = ull_fleet_sign::to_hex(&sha256);
    let file_name = format!("{sha_hex}.bin");
    let size = image.len() as i64;
    write_atomic(releases_dir, &file_name, image).await?;

    let notes = notes.trim().to_string();
    let release = db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO releases (project, version, secure_version, size, sha256, signature,
                                       file_name, build_date, notes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    desc.project_name,
                    desc.version,
                    desc.secure_version,
                    size,
                    sha_hex,
                    ull_fleet_sign::to_hex(&signature),
                    file_name,
                    format!("{} {}", desc.date, desc.time).trim().to_string(),
                    notes,
                    now,
                ],
            )
            .map_err(|err| match err {
                rusqlite::Error::SqliteFailure(e, _)
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    AppError::Conflict("this version is already uploaded".to_string())
                }
                other => other.into(),
            })?;
            Ok(get(conn, conn.last_insert_rowid())?.expect("release was just inserted"))
        })
        .await?;
    Ok(release)
}

async fn write_atomic(dir: &Path, file_name: &str, contents: Vec<u8>) -> AppResult<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let final_path = dir.join(file_name);
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|err| anyhow::anyhow!("getrandom failed: {err}"))?;
    let temp_path = dir.join(format!(
        ".{file_name}.{}.tmp",
        ull_fleet_sign::to_hex(&nonce)
    ));
    let result = async {
        tokio::fs::write(&temp_path, &contents).await?;
        tokio::fs::File::open(&temp_path).await?.sync_all().await?;
        tokio::fs::rename(&temp_path, &final_path).await
    }
    .await;
    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(anyhow::Error::from(err)
            .context(format!("failed to write {}", final_path.display()))
            .into());
    }
    Ok(())
}

pub fn image_path(releases_dir: &Path, release: &Release) -> PathBuf {
    releases_dir.join(&release.file_name)
}

/// Outcome of a deploy, for the UI.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeployOutcome {
    pub targeted: Vec<String>,
    pub already_current: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Sets `release` as the target of the given devices, or of every device of
/// the release's project when `device_ids` is `None`.
pub fn deploy(
    conn: &mut Connection,
    release_id: i64,
    device_ids: Option<&[String]>,
    now: i64,
) -> AppResult<DeployOutcome> {
    let release = get(conn, release_id)?.ok_or(AppError::NotFound)?;
    let tx = conn.transaction()?;
    let devices: Vec<(String, String, String, i64)> = {
        let mut stmt = tx.prepare("SELECT id, project, version, secure_version FROM devices")?;
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?
    };

    let mut outcome = DeployOutcome::default();
    if let Some(ids) = device_ids {
        for id in ids {
            if !devices.iter().any(|(device, ..)| device == id) {
                return Err(AppError::bad_request(format!("unknown device {id}")));
            }
        }
    }

    for (id, project, version, secure_version) in &devices {
        let selected = match device_ids {
            Some(ids) => ids.contains(id),
            None => *project == release.project,
        };
        if !selected {
            continue;
        }
        if *project != release.project {
            outcome.skipped.push((
                id.clone(),
                format!("runs {project}, the release is {}", release.project),
            ));
            continue;
        }
        if release.secure_version < *secure_version {
            outcome.skipped.push((
                id.clone(),
                "the release is older than the running firmware; the device would refuse it"
                    .to_string(),
            ));
            continue;
        }
        let state = if *version == release.version {
            outcome.already_current.push(id.clone());
            UpdateState::None
        } else {
            outcome.targeted.push(id.clone());
            fleet::add_event(
                &tx,
                id,
                now,
                "deploy",
                &format!("Update to {} queued", release.version),
            )?;
            UpdateState::Pending
        };
        tx.execute(
            "UPDATE devices SET target_release_id = ?2, update_state = ?3, update_attempts = 0
             WHERE id = ?1",
            params![id, release.id, state.as_str()],
        )?;
    }
    tx.commit()?;

    if device_ids.is_some()
        && !outcome.skipped.is_empty()
        && outcome.targeted.is_empty()
        && outcome.already_current.is_empty()
    {
        return Err(AppError::bad_request(outcome.skipped[0].1.clone()));
    }
    Ok(outcome)
}

/// Clears a device's target; the device stays on whatever it runs.
pub fn cancel(conn: &Connection, device_id: &str, now: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE devices SET target_release_id = NULL, update_state = 'none', update_attempts = 0
         WHERE id = ?1",
        [device_id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    fleet::add_event(conn, device_id, now, "deploy", "Update cancelled")?;
    Ok(())
}

/// Deletes a release that no device targets.
pub fn delete(conn: &Connection, releases_dir: &Path, id: i64) -> AppResult<()> {
    let release = get(conn, id)?.ok_or(AppError::NotFound)?;
    let in_use: i64 = conn.query_row(
        "SELECT COUNT(*) FROM devices WHERE target_release_id = ?1 AND update_state != 'none'",
        [id],
        |row| row.get(0),
    )?;
    if in_use > 0 {
        return Err(AppError::Conflict(
            "devices are still updating to this release; cancel their updates first".to_string(),
        ));
    }
    conn.execute("DELETE FROM releases WHERE id = ?1", [id])?;
    let shared: i64 = conn.query_row(
        "SELECT COUNT(*) FROM releases WHERE file_name = ?1",
        [&release.file_name],
        |row| row.get(0),
    )?;
    if shared == 0 {
        let _ = std::fs::remove_file(image_path(releases_dir, &release));
    }
    Ok(())
}

/// For each release: (devices running it, devices targeted at it).
pub fn coverage(conn: &Connection) -> AppResult<std::collections::HashMap<i64, (i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT r.id,
                (SELECT COUNT(*) FROM devices d WHERE d.project = r.project AND d.version = r.version),
                (SELECT COUNT(*) FROM devices d WHERE d.target_release_id = r.id AND d.update_state != 'none')
         FROM releases r",
    )?;
    let map = stmt
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<Result<_, _>>()?;
    Ok(map)
}
