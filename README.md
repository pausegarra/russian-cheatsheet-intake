# Russian Cheatsheet Intake

Rust CLI that syncs active OpenRussian words, word relations, and linked example sentences to Russian Cheatsheet API. It creates missing records, skips records that already match, and updates changed words and examples. It uses mark-and-sweep reconciliation: active source identities are removed from in-memory API deletion candidates, and remaining candidates are deleted after a complete pass. Words sync first, then relations and examples; stale words are deleted last.

Disabled OpenRussian words and sentences are outside the active dataset, so their imported API records are deleted. Manual words and examples without `externalId` are preserved. Relations have no source marker; every API relation absent from OpenRussian is deleted, including manually created relations.

Word intake maps `words.audio` to `audioUrl`, `words.usage_en` to `usage`, and `words_forms.form_bare` to source-named API forms. Repeated forms with the same `form_type` are sorted by source position and row ID, then joined with `; `. Word translation objects contain `language`, `text`, and `position`; `managedBy` is internal to the API. Blank types and aspects stay null; unsupported types and form types fail visibly. Manual API translations remain API-managed.

Example intake maps `sentences.id` to API `externalId`, `sentences.ru` to `russian`, nonblank `tl_en` / `tl_de` to English / German translations, and `sentences_words.word_id` to linked API word UUIDs. Translation positions are 0 for English and 1 for German. The source has no sentence audio or contributor fields, so both API values are null. A matching `externalId` with changed content is updated by API UUID; unchanged checksum skips the write. Active examples without links to imported API words are preserved and skipped. Disabled or removed source sentences are deleted from the API. Intake uses `GET /api/v1/examples?externalIdOnly=true` to read only IDs and checksums when building its reconciliation map.

Relation intake maps `words_rels.word_id` and `words_rels.rel_word_id` to API word UUIDs and preserves `words_rels.relation`. It skips self-relations by OpenRussian `externalId`, prepares both directed rows, skips rows whose words are unavailable, and creates only missing keys. It deletes API relations absent from the active source keys, including relations whose API endpoints have null `externalId`; UUID endpoints identify those delete requests. Relation keys use source and related `externalId`, never Russian text. The relation checksum is computed and stored by the API from both Russian word values and relation type; source and API IDs are not part of the checksum. Intake reads all rows from paginated `GET /api/v1/words/relations`, creates batches through `POST /api/v1/words/relations/batch`, and deletes stale batches through `DELETE /api/v1/words/relations/batch`.

## Run

Copy `.env.example` to `.env`, then set the API URL and Keycloak client details. Keycloak client must be confidential, have service accounts enabled, and its service account needs `words#create`, `words#update`, `words#delete`, `examples#create`, `examples#update`, and `examples#delete` roles.

```sh
cp .env.example .env
# Fill RUSSIAN_CHEATSHEET_API_URL, KEYCLOAK_TOKEN_URL, KEYCLOAK_CLIENT_ID, KEYCLOAK_CLIENT_SECRET
cargo run --release
```

Set `RUSSIAN_CHEATSHEET_API_URL` to the API root, such as `http://localhost:8080/api`. Intake appends `/v1` to each API route.

`RUSSIAN_CHEATSHEET_BATCH_SIZE` sets each create, update, or delete batch threshold; default is `1000`. Intake uses five write buffers: word creates, word updates, relation creates, example creates, and example updates. Deletes use `DELETE /api/v1/words/batch`, `DELETE /api/v1/words/relations/batch`, and `DELETE /api/v1/examples/batch`. Word writes finish before relation and example reconciliation so links use returned word UUIDs. Stale relation and example batches finish before stale word batches. Configure the API's `app.batch.max-size` to accept the chosen intake threshold; intake does not cap it. The API relation listing limits pages to 100 items.

Logs include UTC timestamps, source CSV export events, API page progress, word, relation, and example batch outcomes, checksum matches, disabled/unlinked skips, deletion totals, and final counts. HTTP 429 retries resend the whole batch request, honoring numeric `Retry-After` values or using capped backoff, up to four retries per request. Intake downloads and parses each full CSV in memory; it also prepares source words, relations, and examples in memory and does not chunk source iteration. Set `RUST_LOG=debug` before `cargo run --release` to see request starts and cached-token reuse.

Set `KEYCLOAK_TOKEN_URL` to the full OpenID Connect token endpoint URL, usually `https://<host>/realms/<realm>/protocol/openid-connect/token`. Intake uses the `client_credentials` grant. `JwtKeycloakContextHolder.get_token()` checks expiry on every request and renews the token 30 seconds early, using the earlier of JWT `exp` and Keycloak `expires_in`.

The process environment overrides values from `.env`. Set `OPENRUSSIAN_TOGETHERDB_URL` to override the default public OpenRussian database link. The link must use the TogetherDB database path format `/db/{connectionId}/{databaseName}/{tableName}`.

The source uses TogetherDB's public worker export endpoints: one complete CSV export per table, followed by download using its export key. These endpoints serve TogetherDB's web client and are not documented as a stable third-party API, so a TogetherDB change may require updating the source adapter.

## OpenRussian data

OpenRussian dictionary data is licensed CC BY-SA 4.0. Source: [OpenRussian dictionary data](https://en.openrussian.org/dictionary-data). Keep this attribution with redistributed data and publish modifications under the same license.
