# Record ML failure behavior

Type: research
Status: resolved

## Question

What does the v3.3.6 server in `reference/immich` do when every ML service URL fails or is disabled, for face detection, CLIP, OCR, and the person-clustering job that follows a successful detection? Record whether the work is kept and retried, and what the Asset looks like to the app until then. Write the note to `docs/research/ml-failure.md` and link it from this ticket when resolving.

## Answer

Disabled is the config flags, not an empty URL list. A failed `predict` is not kept as a job. The asset is unchanged until something queues the work again. Detail is in [Machine learning when it does not run](../../../docs/research/ml-failure.md).

`machineLearning.urls` must contain at least one URL. The default is `IMMICH_MACHINE_LEARNING_URL` or `http://immich-machine-learning:3003`. A task runs only when `machineLearning.enabled` and that task's `enabled` are both true. Those handlers return `Skipped` before `predict`. `GET /api/server/features` reports `smartSearch`, `facialRecognition`, and `ocr` from the flags alone. URLs that are all down leave the flags true. The app stores `smartSearch` and `ocr`. It does not store `facialRecognition`.

`predict` tries every URL, healthy first, and still tries a URL whose ping failed. If all fail it throws. The worker catches that throw and does not rethrow, so BullMQ completes the job (`attempts` is 1, `removeOnComplete` is true). There is no failure column. Face detection writes `asset_face`, `face_search`, and `facesRecognizedAt` only after success. CLIP writes `smart_search` only after success. OCR writes `asset_ocr`, `ocr_search`, and `ocrAt` only after success. Clustering does not call the ML service. Detection queues it only when a new face row was inserted. URLs being down does not stop clustering of faces that already exist, as long as the facial-recognition flag is on.

Nothing in sync changes for that asset. People, faces, and OCR stay empty because no rows are emitted. With the flags on, a context search gets HTTP 500 from the text embedding call, and similar photos gets HTTP 400 `Asset {id} has no embedding`. OCR text search still returns 200 and does not include the asset. With the flags off, the app hides context search and OCR text search, and a smart-search call is HTTP 400 `Smart search is not enabled`.

The incomplete asset is the retry state. Nightly jobs recluster unassigned machine-learning faces. They do not re-run detection, CLIP, or OCR. The upload thumbnail job requeues those three only when `notify` is set or `source` is `upload`. `regenerate-thumbnail` passes only `{ id }` and does not. `PUT /api/jobs/{name}` command `start` is what selects null `facesRecognizedAt`, null `ocrAt`, and missing `smart_search` rows. Turning the flags back on does not schedule that backlog. `force: true` on faces or OCR deletes the result rows and does not clear the timestamps, so a later non-force run skips those assets. CLIP `force: true` deletes `smart_search`, and a later non-force run does pick them up.
