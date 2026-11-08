# Decide whether an edited still gets a CLIP vector

Type: grilling
Status: resolved

## Question

An edited still has two preview files, one with `isEdited` true and one with `isEdited` false. v3.3.6 `SmartSearch` loads every preview and writes no CLIP vector when that count is not one: `handleEncodeClip` returns failed. [Inventory where smart-search embeddings live](19-inventory-where-smart-search-embeddings-live.md) names the table and the responses a missing vector produces. [Decide whether asset edits are in the Contract](22-decide-whether-asset-edits-are-in-the-contract.md) means an owned still can be edited. Does this server match v3.3.6 and leave that still with no vector, or write a vector from one of the two previews?

## Comments

Recommendation accepted. Two preview files means the CLIP job writes no vector. A vector already stored for that Asset stays until a separate delete removes it.

## Answer

Match v3.3.6. The CLIP job loads every preview file and writes a CLIP vector only when that set has one file. An edited still has two preview files, so the job returns failed and writes nothing. It leaves a vector already stored for that Asset in place.

Saving an edit, or clearing every edit, does not run the job. A vector written while the still had one preview stays through the edit. Clearing the edits leaves one preview, and the next job that does run can write a vector from that file. A run that has already deleted every vector, such as a forced rebuild, leaves the edited still with none.

A similar-photos search on a still with no vector is the 400 named in [Inventory where smart-search embeddings live](19-inventory-where-smart-search-embeddings-live.md). A text search omits that still, as [Decide how this server stores the CLIP vector](26-decide-how-this-server-stores-the-clip-vector.md) already has it. Which preview face detection and OCR read is [Decide which preview face detection and OCR read after an edit](32-decide-which-preview-face-detection-and-ocr-read-after-an-edit.md). A CLIP model change remains [Decide whether a CLIP model change clears stored vectors](35-decide-whether-a-clip-model-change-clears-stored-vectors.md).
