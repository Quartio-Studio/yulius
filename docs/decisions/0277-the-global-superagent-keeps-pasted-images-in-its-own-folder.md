# 0277. The Global Superagent keeps pasted images in its own folder

## Status

Accepted. Specified in [SPEC-433](../../.ai/specs/SPEC-433-i-can-t-paste-to-the-global-superagent-f.md);
task #415 ("I can't paste to the global superadmin"). Extends
[0274](0274-the-global-superagent-takes-its-persons-keys.md), whose terminal took keys and text
pastes but left files out; 0271's root stands.

## Context

A screenshot pasted into the All sandboxes terminal did nothing. A browser cannot hand an image to
Claude Code itself (Claude Code's own image paste reads the server's clipboard), so the web UI
uploads a pasted image or a dropped file as an attachment and pastes its path, which Claude Code
turns into an image (`[Image #1]`); the sandbox Superagent and task terminals have done this since
UI-048. The Global Superagent had no such upload: its attachments cannot go to its host sandbox,
because on a contained server its bubblewrap root is only `users/<id>/superagent`
([0271](0271-a-global-superagent-acts-in-every-sandbox-of-its-person-through-yulius-tools-only.md)),
and its person may lose that host. It has no built-in tools (`--tools ""`): Claude Code 2.1.311
still reads a pasted image path itself (probed in a PTY with its flags), but any other file's path
would only be text to it. Claude Code takes `png`, `jpg`, `jpeg`, `gif` and `webp` as images.

## Decision

- `POST /api/superagent/attachments?name=` stores an upload in the caller's Global Superagent's
  own folder, `users/<id>/superagent/attachments/`, inside its root at the same path, as other
  attachments are stored (streamed, the upload limit and disk check, a free safe name, every folder
  opened without following links, audited `attachment.uploaded` for `superagent:<user>`), and
  answers its absolute path.
- Only images Claude Code reads are taken; any other name is refused (422) before the body is
  read, saying it has no file tools. Only the caller's own running Global Superagent takes files
  (404 otherwise); a disabled account is refused.
- The folder keeps at most 500 files; images older than a week are removed at the next upload, as
  Claude Code read each one when it was pasted.
- The web UI attaches the terminal drop to the All sandboxes view as to This sandbox, with this
  upload and the overlay "Drop an image to send to Claude".

## Alternatives

- **The host sandbox's shell attachments** (`…/shells/{id}/attachments`): a path the contained
  session cannot open, kept in a sandbox its person may lose.
- **Binding the host's attachment folder into its root**: widens a root 0271 keeps to its own
  folder, for every sandbox it moves to.
- **Any file, like the other terminals**: a PDF's path is text a session without file tools cannot
  open; refusing it says so at once.
- **Sending the image through a Yulius tool**: not how a pasted image reaches Claude Code, and a
  model round trip for each paste.

## Consequences

- Pasting a screenshot works the same in both Superagent tabs; text pastes are unchanged.
- The Global Superagent's folder holds up to a week of its person's pasted images, removed only by
  the next upload (a person who stops pasting keeps the last ones until then, within the 500 bound).
- A new Claude Code version that reads more image types needs the list widened in three places
  together: the server's `IMAGE_EXTENSIONS` (`uploads.rs`), the page's `GLOBAL_IMAGE`
  (`filedrop.js`, which refuses other files before sending them) and the mock's route.
