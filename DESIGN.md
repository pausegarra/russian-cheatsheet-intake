# Russian Cheatsheet Intake

Date: 2026-10-06

## Goal

Create a Rust CLI that synchronizes active OpenRussian words and linked example sentences into the Russian Cheatsheet API. Each run scans the source tables, uses API checksums to skip unchanged writes, and uses source IDs to update the same example row when its content changes. No local sync database.

## Scope

- Import `words.bare`, source word type, English translations, verb aspect, word audio URL, English usage text, and forms. Preserve null or blank source type/aspect as API null.
- Import active OpenRussian example sentences with available English and German translations and links to imported words.
- Import OpenRussian word relations in both directions, creating missing rows and deleting API relations absent from the source.
- Store `sentences.id` as the API example `externalId`; use the API example UUID to update an existing sentence when its checksum changes.
- Read public OpenRussian data through TogetherDB; no PostgreSQL/MySQL DSN required.
- Create missing API words and update existing imported words when the API's global word checksum differs.
- Create missing API examples, skip unchanged examples, and update changed examples by API UUID through the API batch routes.
- Buffer changed word and example writes by operation, with request size controlled by `RUSSIAN_CHEATSHEET_BATCH_SIZE`.
- Delete imported API words, examples, and relations absent from the active OpenRussian dataset using in-memory mark-and-sweep candidates.
- Preserve manually created words and examples without `externalId`. Delete every API relation absent from OpenRussian, including manual relations.
- Do not generate Spanish translations in this version.
- Preserve OpenRussian attribution and CC BY-SA licensing information in project documentation.

## Source access and mapping

The source adapter resolves the worker from the public connection ID in the supplied TogetherDB URL, requests one complete CSV export per source table, downloads each export by its export key, and parses the entire file in memory. Intake also prepares source words, relations, and examples in memory; source iteration is not chunked. API writes and deletes are buffered into requests. The export endpoints serve TogetherDB's web client and are not documented as a stable third-party API.

Read these tables:

| Source | Mapping |
| --- | --- |
| `words` | `id` → external ID string; `bare` → API `russian`; `type` → API `type` or null when blank; `audio` → API `audioUrl`; `usage_en` → API `usage`; import only `disabled = false` |
| `translations` | rows with `lang = 'en'`; `tl` and source `position` → English API translations |
| `verbs` | `word_id` and `aspect` → API `aspect`; null or blank remains null |
| `words_forms` | `word_id`, `form_type`, `position`, `id`, and `form_bare` → API `forms`; sort by position then ID and join repeated values with `; ` |
| `words_rels` | `word_id`, `rel_word_id`, and `relation` → directed API word relation; also create the inverse direction |
| `sentences` | `id` → API example `externalId`; `ru` → `russian`; import only `disabled = false` |
| `sentences_translations` | nonblank `tl_en` / `tl_de` → English / German example translations at positions 0 / 1 |
| `sentences_words` | `sentence_id` and `word_id` → source sentence links, mapped to API word UUIDs |

Use `words.bare` and `words_forms.form_bare`, never their accented counterparts. Preserve text exactly; do not trim or normalize Unicode. Send `forms: null` only when no usable source form rows exist. Send the complete API forms object with absent supported fields set to null. If forms exist but source type is blank, report that word as a sync failure because API rejects forms without a type. Preserve nullable `words.audio` and `words.usage_en` as API null.

Send an empty translations array when a word has no English translations. Word translation objects contain `language`, `text`, and source `position`; API uses array index only when position is missing. Preserve positions for display, but exclude them from the checksum. Reject unknown nonblank word types and form types instead of silently mapping or dropping them.

Preserve sentence Russian and translation text exactly. The source does not include contributor or sentence audio fields; send null. Remove repeated links and map only word IDs present in the API word map. Skip a sentence if no linked word maps to an API UUID. The API stores the OpenRussian sentence ID in a nullable, unique `externalId`; manually created examples have null identity.

## API synchronization

Fetch every page from `GET /api/v1/words` and build an in-memory map keyed by `externalId`, storing API `id` and `checksum`. `externalId` is the OpenRussian `words.id` and is the identity key; Russian text and checksum are not identity keys. This handles homographs and identical payloads with distinct OpenRussian IDs.

Load API imported words as deletion candidates keyed by `externalId`. For each active source word, remove that identity from deletion candidates before validation, compare its checksum, and skip matches. Changed words enter one of two independent buffers: creates or updates. Send them through `POST /api/v1/words/batch` and `PUT /api/v1/words/batch`, respectively. After each successful response, validate items in request order and update the in-memory map keyed by `externalId`; updates keep the original API ID. Active words remain protected when preparation or writing fails. Disabled source rows stay absent from the active set. After relation and example cleanup succeeds, delete remaining word candidates through `DELETE /api/v1/words/batch` using API UUIDs.

Relations run after word writes and before examples. Read every page from `GET /api/v1/words/relations` with `perPage=100` (the API maximum), retaining endpoint UUIDs and nullable endpoint external IDs. Compare directed keys `(sourceExternalId, relatedExternalId, relation)` with active source keys. Skip source rows where `word_id == rel_word_id`, map other source IDs to active API UUIDs, prepare both directions, and deduplicate. Create missing relations through `POST /api/v1/words/relations/batch`. Delete every API row whose key is absent, including rows whose endpoint `externalId` is null, through `DELETE /api/v1/words/relations/batch`. Use `RUSSIAN_CHEATSHEET_BATCH_SIZE` for both operations. The API computes and stores each checksum from source and related Russian words plus relation type; IDs never enter that checksum. If a batch outcome is unknown, reload the paginated relation listing and reconcile by endpoint UUID pair plus relation type.

Examples use the same pattern with separate create and update buffers and `POST /api/v1/examples/batch` / `PUT /api/v1/examples/batch`. Remove each active sentence `externalId` from the API example deletion candidates before checking its links; active unlinked examples remain protected. Disabled or absent source examples remain candidates and are deleted through `DELETE /api/v1/examples/batch`. Delete examples and relations before stale words. Word create/update, relation create, example create/update, and resource deletion batches are independent.

`RUSSIAN_CHEATSHEET_BATCH_SIZE` defaults to `1000`. Intake does not cap or range-check the parsed unsigned integer; configure the API's `app.batch.max-size` to accept the chosen threshold. HTTP 429 retries resend a whole batch request, honoring numeric `Retry-After` values or using capped backoff, up to four retries per request. Do not retry individual records through single-record endpoints. For an outcome-unknown batch, reload API IDs and checksums, then reconcile by `externalId` and expected checksum; updates must still have their original API ID. Count unresolved entries as failed and stop when reconciliation cannot establish their outcomes.

For each active source word, normalize the API payload and compute its expected checksum. Queue it for create if no API row has its `externalId`; skip it if the checksum matches; otherwise queue it for update with the existing API ID. Omit the checksum from requests; the API computes and returns it. Batch responses are in request order, and each returned ID/checksum is validated before updating the in-memory map. On an ambiguous batch outcome, reload API pages and reconcile each queued word by `externalId` and expected checksum. Updates must retain their original API ID. Do not retry an uncertain create blindly: the API has a unique index on non-null `externalId`. A process restart needs no local recovery file; the next run discovers completed creates from the API.

Use a confidential Keycloak client with the `client_credentials` grant for the API's `words#create`, `words#update`, and `words#delete` roles. Keep `KEYCLOAK_TOKEN_URL`, `KEYCLOAK_CLIENT_ID`, and `KEYCLOAK_CLIENT_SECRET` in environment configuration; do not place credentials or access tokens in the repository. The `JwtKeycloakContextHolder` checks cached JWT `exp` on every token request and obtains a new token 30 seconds before expiry.

The same service account also needs `examples#create`, `examples#update`, and `examples#delete`. Read every page from `GET /api/v1/examples?externalIdOnly=true`, which populates only `id`, non-null `externalId`, and `checksum`; other DTO fields are null, and linked words/translations are not loaded. For each active linked sentence, skip when checksum matches. Add missing sentences to the create buffer and changed sentences to the update buffer, keeping update requests keyed by the existing API UUID. Update requests omit `externalId`; the API preserves it. After a successful batch response, validate each result and update the example map. Delete stale candidates by API UUID in configured batches. For an ambiguous batch write or delete, re-read the projected listing and reconcile by source ID and original UUID. Do not fall back to individual writes.

## Checksum contract

Keep one API `checksum` per imported word; define it as the canonical checksum of OpenRussian-managed word fields. Relations have dedicated endpoints and must not affect this checksum. Relation creation/deletion must not refresh a word checksum.

The Rust implementation must match the API's canonical word payload. It includes `russian`, `type`, imported translations, `usage`, `audioUrl`, `aspect`, and `forms`; it excludes relations. Each checksum translation contains only `language` and `text`; position remains in stored API data but is not hashed. Sort checksum translations by language, then text, preserving duplicate entries. Serialize compact UTF-8 JSON with sorted property names and null values included, then calculate lowercase SHA-256 hex. Match Jackson's uppercase hexadecimal digits in `\u` escapes for control characters, or hashes diverge for those words. The hash does not include `externalId`, API UUID, audit timestamps, publication state, relations, translation positions, or the checksum itself.

For this version, the expected API projection is:

- `russian`: `words.bare`
- `type`: source `words.type`, or null when source type is blank
- `translations`: English source rows only
- `aspect`: nonblank `verbs.aspect` for verbs; null when absent
- `forms`: complete source-native form object, or null when absent
- `usage`: `words.usage_en`
- `audioUrl`: `words.audio`

Manual translations are preserved by the API and excluded from its checksum. Relations remain independently managed and can change without changing a word checksum. A translation position-only change will not trigger an update; displayed translation ordering may therefore remain unchanged. The API checksum projection already includes forms, usage, and audio URL, so changes to these source fields trigger intake updates.

Example checksums cover Russian text, translations including language/text/position, contributor, audio URL, and sorted linked API word UUIDs. They exclude `externalId`, API UUID, audit fields, and checksum itself. Compare by `externalId` first, then checksum: source identity remains stable while sentence text, translations, or word links change.

## Error handling and reporting

CSV export/download, CSV decoding, and API writes or deletes must fail visibly on unrecoverable HTTP or validation errors. Never sweep after an incomplete source or API read. Emit timestamped INFO logs for complete source table exports, API page progress, each word and relation write/delete outcome, each example outcome including disabled/unlinked skips, and final counts. Emit ERROR logs for failed writes/deletes and DEBUG logs for request starts and cached-token reuse. Report scanned, disabled-skipped, unlinked-skipped, created, updated, unchanged, deleted, and failed counts. Do not convert unknown source types into `other`.

## Verification scope

Production code is checked with the API's `make test` and intake's `cargo test && cargo check`. Do not run the full intake against the local API as part of implementation; it would write the full OpenRussian example dataset.
