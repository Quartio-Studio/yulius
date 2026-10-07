//! Resumable, checksummed chunk upload against the fake server (IMP-001, AC-IMP-1).

use std::fs;
use std::path::PathBuf;

use yulius_contracts::import::{
    FileRef, IMPORT_MANIFEST_VERSION, ImportManifest, RepoDescriptor,
};
use yulius_import::{ApiError, RetryPolicy, Secret, UploadError, Uploader};

use crate::support::TestResult;
use crate::support::server::{FakeServer, TOKEN};

const CHUNK: u64 = 1024;

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 31 % 251) as u8).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

struct Prepared {
    _dir: tempfile::TempDir,
    path: PathBuf,
    file: FileRef,
    data: Vec<u8>,
    manifest: ImportManifest,
}

fn prepare(len: usize) -> TestResult<Prepared> {
    let dir = tempfile::tempdir()?;
    let data = payload(len);
    let path = dir.path().join("repo.bundle");
    fs::write(&path, &data)?;
    let file = FileRef {
        name: "repo.bundle".to_owned(),
        size: data.len() as u64,
        sha256: sha256_hex(&data),
    };
    let manifest = ImportManifest {
        version: IMPORT_MANIFEST_VERSION,
        repo: RepoDescriptor {
            remote_url: "https://github.com/Quartio-Studio/demo".to_owned(),
            owner: "Quartio-Studio".to_owned(),
            name: "demo".to_owned(),
            head_branch: Some("main".to_owned()),
            head_sha: "0".repeat(40),
        },
        bundle: Some(file.clone()),
        snapshot: None,
        sessions: Vec::new(),
        since: "2026-09-21T14:13:20.000Z".to_owned(),
    };
    Ok(Prepared {
        _dir: dir,
        path,
        file,
        data,
        manifest,
    })
}

fn uploader(server: &FakeServer, attempts: u32) -> TestResult<Uploader> {
    Ok(Uploader::new(server.url.clone(), Secret::new(TOKEN))?
        .with_chunk_size(CHUNK)
        .with_retry_policy(RetryPolicy::immediate(attempts)))
}

#[test]
fn interrupted_upload_resumes_from_the_server_offset_without_resending() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(10 * CHUNK as usize + 300)?;
    let client = uploader(&server, 3)?;
    server.with(|state| state.down_after_chunks = Some(4))?;

    let job = client.create_job(&prepared.manifest)?;
    assert_eq!(job.missing.len(), 1);
    assert_eq!(job.missing[0].received_bytes, 0);
    let mut reported = Vec::new();
    let first = client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |done, _| {
        reported.push(done);
    });
    let Err(UploadError::Api(ApiError::Unavailable { attempts: 3, .. })) = first else {
        return Err(format!("expected Unavailable after 3 attempts, got {first:?}").into());
    };
    assert_eq!(reported, [0, CHUNK, 2 * CHUNK, 3 * CHUNK, 4 * CHUNK]);

    // The server comes back; a new run learns the received prefix from job creation.
    let puts_before = server.with(|state| {
        state.down = false;
        state.down_after_chunks = None;
        state.puts.len()
    })?;
    let resumed = client.create_job(&prepared.manifest)?;
    assert_eq!(resumed.missing[0].received_bytes, 4 * CHUNK);
    let sent = client.upload_file(
        &resumed.job_id,
        &prepared.file,
        &prepared.path,
        resumed.missing[0].received_bytes,
        &mut |_, _| {},
    )?;
    assert_eq!(sent, prepared.file.size - 4 * CHUNK);

    server.with(|state| {
        let after: Vec<u64> = state.puts[puts_before..].iter().map(|put| put.offset).collect();
        assert_eq!(after.first(), Some(&(4 * CHUNK)), "resume must not resend the prefix");
        assert!(after.iter().all(|offset| *offset >= 4 * CHUNK));
        assert_eq!(state.stored_bytes, prepared.file.size);
        assert_eq!(state.content.get(&prepared.file.sha256), Some(&prepared.data));
    })?;

    // Everything is stored now: a further run has nothing to upload (IMP-006).
    assert!(client.create_job(&prepared.manifest)?.missing.is_empty());
    Ok(())
}

#[test]
fn lost_responses_are_retried_idempotently() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(5 * CHUNK as usize)?;
    let client = uploader(&server, 3)?;
    server.with(|state| state.lose_response_after_chunk = Some(2))?;

    let job = client.create_job(&prepared.manifest)?;
    let sent = client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |_, _| {})?;

    assert_eq!(sent, prepared.file.size);
    server.with(|state| {
        let statuses: Vec<(u64, u16)> = state.puts.iter().map(|put| (put.offset, put.status)).collect();
        assert_eq!(
            statuses,
            [(0, 200), (CHUNK, 500), (CHUNK, 200), (2 * CHUNK, 200), (3 * CHUNK, 200), (4 * CHUNK, 200)]
        );
        assert_eq!(state.stored_bytes, prepared.file.size);
        assert_eq!(state.content.get(&prepared.file.sha256), Some(&prepared.data));
    })?;
    Ok(())
}

#[test]
fn rejected_chunks_are_not_retried() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(3 * CHUNK as usize)?;
    let client = uploader(&server, 3)?;
    server.with(|state| state.reject_next_chunk = true)?;
    let job = client.create_job(&prepared.manifest)?;
    let result = client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |_, _| {});
    assert!(matches!(
        result,
        Err(UploadError::Api(ApiError::Rejected { status: 422, .. }))
    ));
    assert_eq!(server.with(|state| state.puts.len())?, 1);
    Ok(())
}

#[test]
fn local_changes_are_detected_before_sending() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(2 * CHUNK as usize)?;
    let client = uploader(&server, 3)?;
    let job = client.create_job(&prepared.manifest)?;
    fs::write(&prepared.path, b"grown and different")?;
    let result = client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |_, _| {});
    assert!(matches!(result, Err(UploadError::LocalFileChanged { .. })));
    assert_eq!(server.with(|state| state.puts.len())?, 0);

    // Same size, different bytes: the local digest check refuses after the server-side check.
    let mut tampered = prepared.data.clone();
    tampered[0] ^= 0xff;
    fs::write(&prepared.path, &tampered)?;
    let result = client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |_, _| {});
    assert!(matches!(
        result,
        Err(UploadError::LocalFileChanged { .. } | UploadError::Api(ApiError::Rejected { status: 422, .. }))
    ));
    Ok(())
}

#[test]
fn bad_tokens_and_invalid_manifests_fail_before_upload() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(10)?;
    let wrong = Uploader::new(server.url.clone(), Secret::new("yul_wrong"))?
        .with_retry_policy(RetryPolicy::immediate(3));
    assert!(matches!(
        wrong.create_job(&prepared.manifest),
        Err(UploadError::Api(ApiError::Unauthenticated))
    ));

    let mut invalid = prepared.manifest.clone();
    if let Some(bundle) = invalid.bundle.as_mut() {
        bundle.name = "../escape".to_owned();
    }
    let client = uploader(&server, 3)?;
    assert!(matches!(client.create_job(&invalid), Err(UploadError::Manifest(_))));
    assert_eq!(server.with(|state| state.manifests.len())?, 0);

    let completion = client.complete("job-unknown");
    assert!(matches!(completion, Err(UploadError::Api(ApiError::NotFound(_)))));
    Ok(())
}

#[test]
fn completion_is_refused_until_files_are_complete() -> TestResult {
    let server = FakeServer::start()?;
    let prepared = prepare(3 * CHUNK as usize)?;
    let client = uploader(&server, 1)?;
    let job = client.create_job(&prepared.manifest)?;
    assert!(matches!(
        client.complete(&job.job_id),
        Err(UploadError::Api(ApiError::Conflict(_)))
    ));
    client.upload_file(&job.job_id, &prepared.file, &prepared.path, 0, &mut |_, _| {})?;
    let result = client.complete(&job.job_id)?;
    assert_eq!(result.sandbox_id, "sbx-demo");
    Ok(())
}
