# Decide whether this server serves the official web build

Type: grilling
Status: resolved

## Question

v3.3.6 builds a SvelteKit app and the server process serves those files. `/share/{key}` and `/s/{slug}` return that HTML. [Decide whether a recipient can open a shared link](14-decide-whether-a-recipient-can-open-a-shared-link.md) puts the official web UI in the destination. The Pi does not run Node. Does this server serve that prebuilt v3.3.6 web app, or ship a new web UI with the same screens?

## Comments

Recommendation accepted: this server serves the prebuilt v3.3.6 web app.

## Answer

This server serves the prebuilt v3.3.6 web app. The files are built ahead of time. The Pi serves them and does not run Node.

The v3.3.6 web package uses `@sveltejs/adapter-static` with fallback `index.html` and precompressed assets. A request for a built file is that file. Any other browser request for HTML gets `index.html`. `/share/{key}` and `/s/{slug}` are that same file: the server fills the link preview when the link resolves, and returns 404 when it does not, as [Decide whether a recipient can open a shared link](14-decide-whether-a-recipient-can-open-a-shared-link.md) decided.

The official app is one build, so its other screens load from the same files. `CONTEXT.md` already names that app the Web UI. The calls a recipient's browser makes while a shared link opens are [Inventory how the web UI opens a shared link](24-inventory-how-the-web-ui-opens-a-shared-link.md). Screens beyond that open and beyond the new-user screen stay unspecified.
