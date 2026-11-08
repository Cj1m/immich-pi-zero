# Decide which workflow and plugin responses this server matches

Type: grilling
Status: resolved

## Question

The workflows screen calls `GET`, `POST`, `PUT`, and `DELETE /api/workflows`, `GET /api/workflows/triggers`, `GET /api/workflows/{id}/logs`, `GET /api/plugins/methods`, and `GET /api/plugins/templates`. This server does not run Node. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Which of those calls match v3.3.6, and which change because the Pi cannot run a plugin?

## Comments

Recommendation accepted: these calls match v3.3.6. A plugin is a WASM module run by Extism, not a Node process. Startup imports the shipped core plugin at `plugins/immich-plugin-core`, the same folder v3.3.6 imports. The Pi can host that runtime. There is no Pi difference.

## Answer

`plugin` keeps `id`, `enabled` (default true), `name` (unique), `version`, `title`, `description`, `author`, `wasmBytes` (blob), `templates` (JSON), `sha256hash` (blob), `createdAt`, and `updatedAt`. `name` and `version` are unique together. `plugin_method` keeps `id`, `pluginId`, `name` (unique per plugin), `title`, `description`, `types` (JSON text), `hostFunctions` (default false), `allowedHosts` (JSON text, default empty), `schema` (JSON, null allowed), and `uiHints` (JSON text, default empty).

`workflow` keeps `id`, `ownerId`, `trigger`, `name`, `description`, `createdAt`, `updatedAt`, `updateId`, `enabled` (default true), and `logging` (default false). `workflow_step` keeps `id`, `enabled` (default true), `workflowId`, `pluginMethodId`, `config` (JSON, null allowed), and `order`. `workflow_log` keeps `id`, `createdAt`, `workflowId`, `result`, `workflowStepId`, `triggerDataId`, and `runId`.

Startup imports the core plugin from `plugins/immich-plugin-core`. In development that import is forced. An external folder is imported only when the environment allows it, from `IMMICH_PLUGINS_INSTALL_FOLDER`. A bad manifest is skipped. The stored bytes are what Extism runs.

`GET /api/workflows/triggers` returns 200. The list is `AssetCreate`, `AssetMetadataExtraction`, and `AssetTagged`. Each item is `{trigger, types}` and `types` is `['AssetV1']`. The editor throws on a trigger that is not in this list. `GET /api/plugins/methods` returns 200 of `{key, name, title, hostFunctions, uiHints, description, types, schema}`. `key` is `pluginName#methodName`. The editor reads `key` and `title`. A `trigger` query keeps methods whose types fit that trigger. None is `[]`. `GET /api/plugins/templates` returns 200 of `{key, title, description, trigger, steps, uiHints}`. `key` is `pluginName#templateName`. `steps` is `{method, config, enabled}`. None is `[]` when no plugin has templates.

The workflow document is `id`, `trigger`, `name`, `description`, `createdAt`, `updatedAt`, `enabled`, `logging`, and `steps`. A step is `{method, config, enabled}` and `method` is `pluginName#methodName`. `GET /api/workflows` returns 200 of this user's workflows. None is `[]`. `GET /api/workflows/{id}` returns 200 of one. No read is 400 `Not found or no workflow.read access`. A row that passes that check and then disappears is 400 `Workflow not found`.

`POST /api/workflows` returns 201. The caller is the owner. The response `steps` array is empty even when steps were stored. The following GET returns them. An unknown method is 400 `Unknown method {method}`. A method that does not fit the trigger is 400 `Method "{method}" is incompatible with workflow trigger: "{trigger}"`. `PUT /api/workflows/{id}` returns 200 of the document, including steps. No update is 400 `Not found or no workflow.update access`. The same method errors apply when `steps` is sent. `DELETE /api/workflows/{id}` returns 204. No delete is 400 `Not found or no workflow.delete access`.

`GET /api/workflows/{id}/logs` returns 200, newest first. Default limit is 50. Each entry is `id`, `at`, `result` (`completed`, `halted`, or `error`), `triggerDataId` (omitted when null), and `lastStep` (omitted when the run had no step). `lastStep` is `{index, method}` and that `method` is `{pluginId}#{methodName}`. None is `[]`. A log row is written only when `logging` is true. No logs access is 400 `Not found or no workflow.logs access`.

An enabled workflow runs when an asset is created, metadata is extracted (except a sidecar write), or a tag is added. A disabled workflow does not run. A disabled step is skipped. A step that says continue false logs `halted` and stops. A thrown step logs `error` and stops. A finished run logs `completed`. A plugin write goes through the asset, album, or tag update those screens already use, so the phone sees the same sync. A host call is refused unless that method set `hostFunctions` true.

Unsigned is 401. A shared link is 403 `Forbidden`. The phone does not sync workflows.
