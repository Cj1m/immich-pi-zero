# Decide which preview face detection and OCR read after an edit

Type: grilling
Status: resolved

## Question

[Decide whether asset edits are in the Contract](22-decide-whether-asset-edits-are-in-the-contract.md) gives an owned still two preview files. v3.3.6 face detection takes the edited preview: `isEdited` descending, one row. OCR takes the unedited preview: `withFilePath` defaults `isEdited` to false. Which preview does this server send for face detection, and which for OCR?

## Comments

Recommendation accepted. Face detection sends the edited preview when that file exists, and the unedited preview otherwise. OCR sends the unedited preview.

## Answer

Match v3.3.6. Face detection sends the edited preview when that file exists, and the unedited preview when it is the only one. OCR sends the unedited preview.

Saving an Edit writes the edited preview and does not queue face detection or OCR. The rows from the last run stay. A later face run reads the edited preview when that file exists and replaces the machine-learning faces, as [Record ML failure behavior](07-record-ml-failure-behavior.md) describes. A later OCR run reads the unedited preview and replaces that Asset's OCR rows. An edited still keeps its unedited preview, so that OCR run has a file.

The Web UI draws Face boxes and OCR regions on the photo it loaded with `edited=true`. The mobile app draws OCR regions on that photo. It does not draw Face boxes. A person thumbnail for a still stays a crop of the Original, using the box from the preview face detection read, so a crop Edit moves that thumbnail off the face when the box came from the edited preview. OCR regions stay in the unedited photo's coordinates and are drawn on the edited photo. Search still finds text the crop removed.
