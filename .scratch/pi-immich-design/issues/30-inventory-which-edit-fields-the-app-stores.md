# Inventory which edit fields the app stores

Type: research
Status: resolved

## Question

[Decide whether asset edits are in the Contract](22-decide-whether-asset-edits-are-in-the-contract.md) puts the editor in the Contract. On a single owned still, the viewer shows Edit. Saving a crop, rotate, or mirror writes the edit, then the edited thumbnail and preview. The file paths are already in [Inventory where rendition files live](13-inventory-where-rendition-files-live.md). Which HTTP calls and sync fields does the v3.3.6 app use to open the editor, save an edit, clear an edit, and show the edited still? Name the method, path, and the fields the app requires. Include `AssetEditsV1` and the delete line. Say what a missing field does to the editor and to the viewer. Write the note to `docs/research/asset-edits.md` and link it from this ticket when resolving.

## Answer

The app does not call `GET /api/assets/{id}/edits`. Edit is shown for one owned still when the server is at least 2.6.0. Opening reads the local edit rows and the exif row, and paints the canvas with `edited=false` unless a local file stands in. A missing exif row does not open the editor. A missing width or height still opens it, and the stored crop is not applied.

Save sends `PUT /api/assets/{id}/edits` with `edits[]` of `action` and `parameters`. Crop is `x`, `y`, `width`, `height`. Rotate is `angle`. Mirror is `axis`. An empty list is `DELETE /api/assets/{id}/edits`. The app does not read the response body. A thrown call stays on the editor. A completed call pops, then waits up to 10 seconds for `AssetEditReadyV2`, which replaces the local rows. A timeout still counts as success.

The tile and the viewer request `edited=true`. `isEdited` on `AssetV2` only decides whether a local file may stand in. `AssetEditsV1` syncs `AssetEditDeleteV1` (`editId`) and then `AssetEditV1` (`id`, `assetId`, `action`, `parameters`, `sequence`). The app stores those five. A missing required sync field fails that batch and does not ack it. A `parameters` object with the wrong keys lands, and fails the next time the editor reads the row.

The calls, the stored columns, and those screens are in [Asset edits](../../../docs/research/asset-edits.md). Which of those fields this server keeps is [Decide which edit fields this server stores](37-decide-which-edit-fields-this-server-stores.md).
