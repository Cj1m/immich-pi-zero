# Decide whether OCR is in the Contract

Type: grilling
Status: resolved

## Question

The viewer draws OCR boxes and the selected box's text from local `AssetOcrV1` rows. Search can send that text as `ocr` on `POST /api/search/metadata`. The OCR control appears only when server features say `ocr`. [Inventory how the app reads ML results](06-inventory-how-the-app-reads-ml-results.md) records the calls. The Contract names faces, people, and search results, and does not name OCR. Is the overlay, and search by that text, part of the Contract?

## Comments

Recommendation accepted: the overlay and search by that text are both in the Contract.

## Answer

OCR is in the Contract. On an image, the viewer draws each visible region and, once that region is selected, its text. The toggle appears when that remote image has at least one visible row. The overlay is drawn when the asset has width and height. The rows come from `AssetOcrV1` on `POST /api/sync/stream`.

Search by that text is in the Contract. The search screen shows the OCR text mode when server features say `ocr`. Submitting it sends `ocr` on `POST /api/search/metadata`. The server matches that string against one search-text row for the Asset. The success path does not call `GET /api/assets/{id}/ocr`.

Each `AssetOcrV1` carries `id`, `assetId`, the eight corners (`x1` through `y4`), `text`, `boxScore`, `textScore`, and `isVisible`. Every one of those is required. A missing field throws before the row is stored. The overlay reads the corners and the text, and keeps a row when `isVisible` is true. The scores travel so the row can land. `AssetOcrDeleteV1` removes a row by id.

The recognition is an ML service call on the preview. The Pi stores the rows and answers the search. None of the eight constraints in [Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) remove this. The search mode follows the `ocr` feature flag. The overlay follows the rows.
