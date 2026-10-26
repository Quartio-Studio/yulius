# 0219. Uploads take 1 GiB by default and stop short of a critical disk

## Status

Accepted. Specified in [SPEC-325](../../.ai/specs/SPEC-325-hey-i-have-a-few-bugs-i-encontered-throu.md).
Changes the default size of EDT-006 and UI-048 (SPEC-001) from 25 MiB to 1 GiB. Uses the data
disk levels of SPEC-276.

## Context

People hand Claude Code a screen recording of a bug and let it extract frames with `ffmpeg`.
A recording of a few minutes is often hundreds of MB. Yulius refused uploads over 25 MiB by
default (`YULIUS_UPLOAD_MAX_MB`, configurable up to 1024), and the documented reverse proxy
refused request bodies over 32 MB. Uploads already stream to disk through a bounded buffer: the
size costs disk, not server memory. The data disk also holds the database and every sandbox.
With 500 attachments per task, 1 GiB files could fill it, and until now only a disk that was
already full was caught, during the write.

Two web views also read a non-image prompt attachment into the page as a `Blob`, with a 120 s
total timeout. A 1 GiB file would need 1 GiB of page memory and a link of about 9 MB/s.

## Decision

- `YULIUS_UPLOAD_MAX_MB` defaults to 1024 (1 GiB), its highest value. The web UI uses the same
  fallback, and sizes from 1 GiB read in GB ("1 GB").
- An upload whose declared length would bring the data disk to its critical level (95 % used,
  SPEC-276) is refused before anything is created. It is a 422 that names the free space, like a
  file over the limit, so the web UI shows it and does not retry it as it retries a 429. When the
  disk size is unknown, nothing is refused.
- A non-image attachment opened from the original prompt or a queued request is downloaded by the
  browser from its link (`Content-Disposition: attachment`, already sent), after a HEAD request
  that turns a deleted file into the usual "not on the server" message.
- The documented proxies allow 1100 MB bodies (1 GiB with room for the request). An upgrade note
  tells operators to raise an existing proxy's limit, which a merge cannot change.

## Alternatives

- **Keep 25 MiB and document the setting.** Operators would have to find and raise two limits for
  the most common size of recording. The default should fit the way the product is used.
- **A per-task or per-user byte quota.** It would refuse a second recording on a disk with plenty
  of room and needs a new setting. The disk level protects what actually breaks, the database and
  the other tasks, with the existing threshold.
- **Count uploads still in progress against the free space.** Each upload is checked alone, so at
  most 8 uploads (8 GiB) can pass the check together. The 5 % margin absorbs that on the disks
  Yulius runs on, and a disk that does fill still ends the upload with `Capacity`.
- **A longer timeout for the in-page read.** The whole file would still sit in page memory.

## Consequences

- A default instance takes a 1 GB recording once its proxy allows the body. Until then the proxy
  answers 413 and the web UI says "larger than the server accepts".
- 1 GiB at local disk speed takes about 20 s through the server (measured in a test, SPEC-325).
  Over a slow link the upload is bounded only by its 60 s idle limit, not by its total time.
- Attachments can now take far more of the data disk. They still go with their task, a draft
  after a week and a shell's when it ends. The disk upkeep does not remove them.
