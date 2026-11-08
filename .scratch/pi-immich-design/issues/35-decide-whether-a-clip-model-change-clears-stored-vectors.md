# Decide whether a CLIP model change clears stored vectors

Type: grilling
Status: resolved

## Question

[Decide how this server stores the CLIP vector](26-decide-how-this-server-stores-the-clip-vector.md) stores one float32 blob per Asset. The blob's length is the length the ML service returned for the configured model. In v3.3.6, a change of CLIP model name deletes every `smart_search` row, and a change of dimension deletes every row and alters the column. After that, a similar-photos search is the missing-vector 400, and a text search is the empty 200 page, until the encode job writes new rows. [Search embeddings](../../../docs/research/search-embeddings.md) records those steps. When the configured CLIP model changes, does this server delete every stored CLIP vector?

## Comments

Recommendation accepted, including the follow-ups.

## Answer

Match v3.3.6. A change of the configured CLIP model deletes every stored CLIP vector and does not queue a rewrite. Until a later encode writes a new blob, a similar-photos search is the missing-vector 400 named in [Inventory where smart-search embeddings live](19-inventory-where-smart-search-embeddings-live.md), and a text search is the empty 200 page named in [Decide how this server stores the CLIP vector](26-decide-how-this-server-stores-the-clip-vector.md).

The configured model is `machineLearning.clip.modelName`. The default is `ViT-B-32__openai`, whose dimension is 512. The dimension of a name is the v3.3.6 `CLIP_MODEL_INFO` entry, after the same cleaning: the last `/` segment, with `:` replaced by `_`. A name with no entry is refused. That save answers 400 `{ message: "Unknown CLIP model: <name>. Please check the model name for typos and confirm this is a supported model." }` and leaves every stored vector in place.

The delete runs only when smart search is enabled, which is `machineLearning.enabled` and `machineLearning.clip.enabled`. Turning smart search off leaves the blobs, including when that same save changes the model name. A later save that turns it back on compares the name already stored, so a same-dimension change made while smart search was off leaves the blobs.

On a config update while the server is running, a new model name deletes every CLIP vector, including when the dimension stays the same. A stored blob whose length differs from the new model's dimension deletes every CLIP vector as well.

On startup there is no previous name to compare. The server deletes every CLIP vector when a stored blob's length differs from the configured model's dimension. When every stored length matches, the blobs stay, including when the model name changed while the process was down. Search then ranks those blobs as they are.

The delete covers every CLIP vector, including an Asset that is trashed or hidden. Face embeddings and OCR rows stay. The blob stays one float32 value per Asset, with no model name stored on it, and there is no vector column to alter.

An encode writes its blob when the configured model name is still the name that call used and the returned length is that model's dimension. Any other return writes nothing.

A forced rebuild stays the manual smart-search queue start. That action is separate from this model change. It still deletes every vector and then queues the encode, as [Decide whether an edited still gets a CLIP vector](31-decide-whether-an-edited-still-gets-a-clip-vector.md) already names. How an operator edits the model name stays with the admin screens in the map's Not yet specified section.
