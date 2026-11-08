# Decide whether a recipient can open a shared link

Type: grilling
Status: resolved

## Question

[Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) shows the official app creates, lists, edits, and deletes shared links, then copies a URL (`/s/{slug}`, or `/share/{key}` when there is no slug). It never calls `GET /api/shared-links/me`, `GET /api/shared-links/{id}`, or `POST /api/shared-links/login`. A recipient opening that URL is not this app. The web UI is out of scope. Is resolving a shared link part of the Contract, or does the Contract stop at the calls the app makes?

## Comments

The web UI is in upstream v3.3.6. The aim is feature parity with that server, dropping a feature only when the Pi cannot support it. The glossary still defines the Contract as the official mobile app's success path, and lists full API and OpenAPI parity as words to avoid. Whether the destination grows to include the web UI is the open question.

Recommendation accepted: the destination includes the official mobile app and the official web UI. A recipient opening `/s/{slug}` or `/share/{key}` is in the destination. The Pi still does not transcode, run models, or use Postgres, Redis, or Node at runtime, and one User uploads at a time.

## Answer

A recipient opens a shared link in the official web UI. That open is in the destination. The mobile app still only copies `/s/{slug}` or `/share/{key}`, and those calls stay what [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) recorded.

v3.3.6 ships a SvelteKit web app. The server image builds it and the same process serves the files. `/share/{key}` and `/s/{slug}` are pages of that app. The server returns the app's HTML, fills in the link preview when the link resolves, and returns 404 when it does not. The browser runs the interface. The Pi serves those files. It does not run Node.

The destination is a locked design for a server the official mobile app and that web UI can use. The Contract stays the mobile app's success path. The Web UI is its own term in `CONTEXT.md`.

[Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) still holds for mobile responses. Its "no web UI" constraint does not hold for this server: the server serves the Web UI. These Pi limits stay: no video transcode, no models on the Pi, no Postgres, Redis, or Node at runtime, and one active uploader.

The calls the browser makes while that page opens are not inventoried here.
