# Decide which Pi constraints the app can see

Type: grilling
Status: resolved
Blocked by: 01, 02, 03, 04, 05, 06, 07

## Question

Given the inventories from the blocking tickets, which Pi constraints change a response the official mobile app will see, and which can differ from v3.3.6 without the app noticing? The constraints are SQLite instead of Postgres, Originals in S3, no Redis, no video transcode, no web UI, no model running on the Pi, resident memory under 200 MB, and one active uploader.

## Comments

A constraint counts only when a success-path call returns a different status or body, or the call is missing. Latency and process layout do not count.

`GET /api/assets/{id}/video/playback` streaming the Original is the same success-path response. No video transcode stays invisible. Status stays 200, or 206 when the player sends `Range`.

Each of the three in-flight `POST /api/assets` calls from one phone gets HTTP 200 or 201 with a string `id`.

## Answer

None of the eight constraints change a success-path response. Each can differ from v3.3.6 without the app noticing, as long as the calls in the inventories keep the same status and body.

- SQLite instead of Postgres is invisible. The app sees the JSON and file bodies those calls already require.
- Originals in S3 are invisible. The app receives the Original bytes with the same 200, or 206 when it sends `Range`.
- No Redis is invisible. The app never speaks to Redis. The websocket is not a success-path response, so this decision does not require it.
- No video transcode is invisible. `GET /api/assets/{id}/video/playback` streams the Original. That is the response v3.3.6 already returns when no encoded file exists. Status stays 200, or 206 when the player sends `Range`.
- No web UI is invisible. The success path never opens it.
- No model on the Pi is invisible. Faces, people, and search still arrive through the same sync and search calls.
- Resident memory under 200 MB is invisible. The budget does not change a success-path status or body.
- One active uploader is one User. The foreground backup's three in-flight `POST /api/assets` calls each return 200 or 201 with a string `id`.
