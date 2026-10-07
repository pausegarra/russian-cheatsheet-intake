# Russian Cheatsheet Intake

Rust CLI that syncs active OpenRussian words and linked example sentences to Russian Cheatsheet API. It creates missing records, skips records with matching checksums, and updates changed records. Words sync first so each example can link to API word UUIDs. Disabled words and sentences are skipped. Intake never deletes API words or examples.

Word intake maps `words.audio` to `audioUrl`, `words.usage_en` to `usage`, and `words_forms.form_bare` to source-named API forms. Repeated forms with the same `form_type` are sorted by source position and row ID, then joined with `; `. Word translation objects contain `language`, `text`, and `position`; `managedBy` is internal to the API. Blank types and aspects stay null; unsupported types and form types fail visibly. Manual API translations remain API-managed.

Example intake maps `sentences.id` to API `externalId`, `sentences.ru` to `russian`, nonblank `tl_en` / `tl_de` to English / German translations, and `sentences_words.word_id` to linked API word UUIDs. Translation positions are 0 for English and 1 for German. The source has no sentence audio or contributor fields, so both API values are null. A matching `externalId` with changed content is updated by API UUID; unchanged checksum skips the write. Examples without links to imported API words are skipped. Disabled or removed source sentences do not delete existing API examples. Intake uses `GET /api/examples?externalIdOnly=true` to read only IDs and checksums when building its reconciliation map.

## Run

Copy `.env.example` to `.env`, then set the API URL and Keycloak client details. Keycloak client must be confidential, have service accounts enabled, and its service account needs `words#create`, `words#update`, `examples#create`, and `examples#update` roles.

```sh
cp .env.example .env
# Fill RUSSIAN_CHEATSHEET_API_URL, KEYCLOAK_TOKEN_URL, KEYCLOAK_CLIENT_ID, KEYCLOAK_CLIENT_SECRET
cargo run --release
```

`RUSSIAN_CHEATSHEET_BATCH_SIZE` sets each write batch's maximum size. Valid range is `1`–`1000`; default is `1000`. Intake uses four independent buffers: word creates, word updates, example creates, and example updates. It flushes a buffer at the threshold and sends each nonempty remainder after its source pass. Writes use `POST` and `PUT /api/words/batch` and `POST` and `PUT /api/examples/batch`; all word batches finish before example batches so examples use the returned word UUIDs. If the API's `app.batch.max-size` is configured below `1000`, set the intake value at or below that lower server limit.

Logs include UTC timestamps, source CSV export events, API page progress, word and example batch outcomes, checksum matches, disabled/unlinked skips, and final counts. HTTP 429 retries resend the whole batch request, honoring numeric `Retry-After` values or using capped backoff, up to four retries per request. Intake downloads and parses each full CSV in memory; it also prepares source words in memory and does not chunk source iteration. Set `RUST_LOG=debug` before `cargo run --release` to see request starts and cached-token reuse.

Set `KEYCLOAK_TOKEN_URL` to the full OpenID Connect token endpoint URL, usually `https://<host>/realms/<realm>/protocol/openid-connect/token`. Intake uses the `client_credentials` grant. `JwtKeycloakContextHolder.get_token()` checks expiry on every request and renews the token 30 seconds early, using the earlier of JWT `exp` and Keycloak `expires_in`.

The process environment overrides values from `.env`. Set `OPENRUSSIAN_TOGETHERDB_URL` to override the default public OpenRussian database link. The link must use the TogetherDB database path format `/db/{connectionId}/{databaseName}/{tableName}`.

The source uses TogetherDB's public worker export endpoints: one complete CSV export per table, followed by download using its export key. These endpoints serve TogetherDB's web client and are not documented as a stable third-party API, so a TogetherDB change may require updating the source adapter.

## OpenRussian data

OpenRussian dictionary data is licensed CC BY-SA 4.0. Source: [OpenRussian dictionary data](https://en.openrussian.org/dictionary-data). Keep this attribution with redistributed data and publish modifications under the same license.
