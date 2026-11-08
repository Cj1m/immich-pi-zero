# Decide how an operator sets the machine-learning URL and object-storage credentials

Type: grilling
Status: resolved

## Question

The settings page loads `GET /api/system-config` and `GET /api/system-config/defaults`, and a save sends the whole document back with `PUT /api/system-config`. [Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) left that body unspecified. The machine-learning URL is `machineLearning.urls` on that document. v3.3.6 has no bucket, endpoint, or key field. [Decide how the first User is created](10-decide-how-the-first-user-is-created.md) already puts the first admin in the same config channel as the object-storage credentials. Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. How does an operator set the machine-learning URL, and where do the object-storage credentials live?

## Comments

Recommendation accepted, including the follow-ups. The inventory is [Remaining to close](../../../docs/research/remaining-to-close.md).

## Answer

The operator sets the machine-learning URL in the admin config document. Object-storage credentials stay outside that document. They are environment variables, in the same config channel as the first admin's email, name, and password. The web has no field for them.

`GET /api/system-config`, `GET /api/system-config/defaults`, and `PUT /api/system-config` match v3.3.6. `GET` and `PUT /api/admin/config`, and `GET /api/admin/config/defaults`, are that same document. The stock web does not call those three. An admin session is required on all six. A missing session is 401 `Authentication required`. A non-admin is 403 `Forbidden`. There is no `DELETE`. Reset copies the defaults into the form and saves with the same `PUT`.

Each `GET` and `PUT` returns 200 and the v3.3.6 `AdminConfigDto`. The top-level keys are `backup`, `ffmpeg`, `integrityChecks`, `job`, `logging`, `machineLearning`, `map`, `reverseGeocoding`, `metadata`, `oauth`, `passwordLogin`, `storageTemplate`, `image`, `newVersionCheck`, `nightlyTasks`, `trash`, `theme`, `library`, `notifications`, `templates`, `server`, and `user`. A missing key throws when that settings accordion opens, so every key is present. The nested defaults are the frozen object in `reference/immich/server/src/dtos/config.dto.ts` at v3.3.6. `GET /api/server/config`, `GET /api/config`, and `GET /api/public/config` stay the other documents already decided. Fields those documents already read keep coming from this one. Server `isOnboarded` stays the onboarding metadata flag.

The machine-learning URL is `machineLearning.urls`, a list of at least one string. The default list is `[IMMICH_MACHINE_LEARNING_URL]`, or `["http://immich-machine-learning:3003"]` when that variable is unset. `machineLearning.enabled` defaults to true unless `IMMICH_MACHINE_LEARNING_ENABLED` is the string `false`. Both variables are read when the defaults are built. A saved `machineLearning.urls` or `machineLearning.enabled` replaces that default on `GET /api/system-config` and stays off `GET /api/system-config/defaults`. The other `machineLearning` defaults stay the v3.3.6 values: availability checks on, timeout 2000, interval 30000; CLIP on, model `ViT-B-32__openai`; duplicate detection on, `maxDistance` 0.01; facial recognition on, model `buffalo_l`, `minScore` 0.7, `maxDistance` 0.5, `minFaces` 3; OCR on, model `PP-OCRv5_mobile`, `minDetectionScore` 0.5, `minRecognitionScore` 0.8, `maxResolution` 736. Saving `machineLearning.clip.modelName` still follows [Decide whether a CLIP model change clears stored vectors](35-decide-whether-a-clip-model-change-clears-stored-vectors.md). An unknown model name is 400 with that ticket's message, and the previous document stays.

`PUT` accepts the whole document. The server stores only the keys that differ from the defaults, under system metadata key `system-config`. An empty string, null, or a value equal to the default is not stored. `GET` merges the stored partial onto the defaults. A body that is not this document is 400. `IMMICH_LOG_LEVEL`, when set, makes a logging change 400 `Logging cannot be changed while the environment variable IMMICH_LOG_LEVEL is set.` A storage template that fails the v3.3.6 render check is 400 `Invalid storage template`. Enabling SMTP, or changing it while it is enabled, verifies the transport. A failed verify is 400 `Invalid SMTP configuration`.

`IMMICH_CONFIG_FILE`, when set, is a YAML partial that replaces the metadata row. The settings form is then read-only, import is hidden, and `PUT` is 400 `Cannot update configuration while IMMICH_CONFIG_FILE is in use`. `GET /api/server/features` reports `configFile` from that variable. This server does not use that file for the bucket. Left unset, the form stays editable.

`ffmpeg` and `job.videoConversion` are in the document so those accordions open. Saving them does not start a transcode. Playback stays the Original, as already decided. `ffmpeg.realtime.enabled` is stored and returned. `GET /api/server/features` keeps `realtimeTranscoding` false, because the Pi cannot transcode. The viewer does not request HLS.

`notifications.smtp` is stored and returned. `GET /api/server/features` keeps `email` false, and no mail is sent, as [Decide how a later User is created](20-decide-how-a-later-user-is-created.md) already set. The welcome switch stays hidden.

`storageTemplate.enabled`, `template`, and `hashVerificationEnabled` are stored and returned. `GET /api/system-config/storage-template-options` is 200 and the v3.3.6 token list plus preset list from `storage-template.service.ts`. The form's preview substitutes those tokens in the browser. The saved template does not change an object key. `library.scan` and `library.watch` are stored and returned. They do not set the bucket.

`oauth.enabled` defaults to false and `passwordLogin.enabled` defaults to true. A saved value is what `GET /api/server/features` and `GET /api/public/config` report for those flags. The calls a true `oauth` flag would start stay unresolved.

The bucket credentials are read at startup. `IMMICH_S3_ENDPOINT`, `IMMICH_S3_BUCKET`, `IMMICH_S3_ACCESS_KEY_ID`, and `IMMICH_S3_SECRET_ACCESS_KEY` are required. If one is missing, the process refuses to start and names the missing variable. `IMMICH_S3_REGION` is optional and defaults to `us-east-1`. `IMMICH_S3_FORCE_PATH_STYLE` is optional and defaults to false. These six are not keys of `AdminConfigDto`, not keys of the `system-config` metadata row, and not keys of `IMMICH_CONFIG_FILE`. The object key itself stays out of this document.
