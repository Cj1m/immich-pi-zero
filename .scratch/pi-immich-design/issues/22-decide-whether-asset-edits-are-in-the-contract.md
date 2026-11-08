# Decide whether asset edits are in the Contract

Type: grilling
Status: resolved

## Question

The viewer bottom bar includes an edit action. Saving an edit writes `thumbnail` and `preview` rows with `isEdited` true, and `GET /api/assets/{id}/thumbnail` prefers that row when `edited=true`. The viewer sends `edited=true` by default. [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) records the files and `AssetEditThumbnailGeneration`. The encoded video row is always `isEdited` false. Is the editor part of the Contract?

## Comments

Recommendation accepted: the editor is in the Contract. Crop, rotate, and mirror on an owned still are in, and so are the edited thumbnail and preview the viewer loads afterward.

## Answer

The editor is in the Contract. On a single owned still, the viewer shows Edit when the server reports at least 2.6.0. A still is an image that is not a motion photo and not animated. The editor applies a crop, a rotate, or a mirror.

Saving writes that edit, then the edited thumbnail and preview. The viewer loads those files. Their paths are the `isEdited` thumbnail and preview already named in [Inventory where rendition files live](13-inventory-where-rendition-files-live.md). The unedited thumbnail and preview stay, because the editor opens the still with `edited=false`.

A video, a live photo, a panorama, a GIF, and an SVG stay uneditable, matching v3.3.6.

None of the eight constraints in [Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) remove this. The edited files are still images on the Pi, the same work as the unedited thumbnail and preview.

The fields the app stores, and what a missing field does, are [Inventory which edit fields the app stores](30-inventory-which-edit-fields-the-app-stores.md). Whether an edited still gets a CLIP vector is [Decide whether an edited still gets a CLIP vector](31-decide-whether-an-edited-still-gets-a-clip-vector.md). Which preview face detection and OCR read after an edit is [Decide which preview face detection and OCR read after an edit](32-decide-which-preview-face-detection-and-ocr-read-after-an-edit.md).
