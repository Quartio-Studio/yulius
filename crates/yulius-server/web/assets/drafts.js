// The new-task composer's drafts (SPEC-026): what a person put in a sandbox's composer (the
// prompt, the attached files and the choices on its edge) waits here while they visit other
// pages, and is cleared when the task starts.
//
// A draft is `{ text, mode, workflowId, afterTaskId, startRule, attached }`; `mode` and
// `workflowId` are null until chosen (the composer's defaults apply). Each attachment is
// `{ path, name, type, at, file? }`: the file was uploaded at once (SPEC-014), so only its server
// path is needed to start the task; `file` (this page only) draws its thumbnail.

/** sandbox id → its draft, for this page. */
const drafts = new Map();

function emptyDraft() {
  return { text: '', mode: null, workflowId: null, afterTaskId: '', startRule: '', attached: [] };
}

/** The live draft of a sandbox's composer: the composer reads and changes it in place. */
export function draftFor(sandboxId) {
  let draft = drafts.get(sandboxId);
  if (!draft) {
    draft = emptyDraft();
    drafts.set(sandboxId, draft);
  }
  return draft;
}

/** Records a change of the draft (the composer calls it after each edit). */
export function saveDraft(_sandboxId) {
  // Kept in memory for now.
}

/** Empties a sandbox's draft in place (its task started). */
export function clearDraft(sandboxId) {
  Object.assign(draftFor(sandboxId), emptyDraft());
}
