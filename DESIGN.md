# Russian Cheatsheet Intake

Date: 2026-10-06

## Goal

Create a Rust CLI that synchronizes active OpenRussian words and linked example sentences into the Russian Cheatsheet API. Each run scans the source tables, uses API checksums to skip unchanged writes, and uses source IDs to update the same example row when its content changes. No local sync database.

## Scope

- Import `words.bare`, source word type, English translations, verb aspect, word audio URL, English usage text, and forms. Preserve null or blank source type/aspect as API null.
- Import active OpenRussian example sentences with available English and German translations and links to imported words.
- Store `sentences.id` as the API example `externalId`; use the API example UUID to update an existing sentence when its checksum changes.
- Read public OpenRussian data through TogetherDB; no PostgreSQL/MySQL DSN required.
- Create missing API words and update existing imported words when the API's global word checksum differs.
- Create missing API examples, skip unchanged examples, and update changed examples by API UUID. Do not add a batch endpoint.
- Skip disabled OpenRussian rows. Never delete API words.
- Never delete API examples when an OpenRussian sentence is disabled or removed; the API has no example deletion endpoint.
- Do not generate Spanish translations in this version.
- Do not import word relations in this version.
- Preserve OpenRussian attribution and CC BY-SA licensing information in project documentation.

## Source access and mapping

The source adapter resolves the worker from the public connection ID in the supplied TogetherDB URL, requests one complete CSV export per source table, downloads each export by its export key, and parses the entire file in memory. No row paging or chunks. The export endpoints serve TogetherDB's web client and are not documented as a stable third-party API.

Read these tables:

| Source | Mapping |
| --- | --- |
| `words` | `id` → external ID string; `bare` → API `russian`; `type` → API `type` or null when blank; `audio` → API `audioUrl`; `usage_en` → API `usage`; import only `disabled = false` |
| `translations` | rows with `lang = 'en'`; `tl` and source `position` → English API translations |
| `verbs` | `word_id` and `aspect` → API `aspect`; null or blank remains null |
| `words_forms` | `word_id`, `form_type`, `position`, `id`, and `form_bare` → API `forms`; sort by position then ID and join repeated values with `; ` |
| `sentences` | `id` → API example `externalId`; `ru` → `russian`; import only `disabled = false` |
| `sentences_translations` | nonblank `tl_en` / `tl_de` → English / German example translations at positions 0 / 1 |
| `sentences_words` | `sentence_id` and `word_id` → source sentence links, mapped to API word UUIDs |

Use `words.bare` and `words_forms.form_bare`, never their accented counterparts. Preserve text exactly; do not trim or normalize Unicode. Send `forms: null` only when no usable source form rows exist. Send the complete API forms object with absent supported fields set to null. If forms exist but source type is blank, report that word as a sync failure because API rejects forms without a type. Preserve nullable `words.audio` and `words.usage_en` as API null.

Send an empty translations array when a word has no English translations. Word translation objects contain `language`, `text`, and source `position`; API uses array index only when position is missing. Preserve positions for display, but exclude them from the checksum. Reject unknown nonblank word types and form types instead of silently mapping or dropping them.

Preserve sentence Russian and translation text exactly. The source does not include contributor or sentence audio fields; send null. Remove repeated links and map only word IDs present in the API word map. Skip a sentence if no linked word maps to an API UUID. The API stores the OpenRussian sentence ID in a nullable, unique `externalId`; manually created examples have null identity.

## API synchronization

Fetch every page from `GET /api/words` and build an in-memory map keyed by `externalId`, storing API `id` and `checksum`. `externalId` is the OpenRussian `words.id` and is the identity key; Russian text and checksum are not identity keys. This handles homographs and identical payloads with distinct OpenRussian IDs.

For each active source word:

1. Normalize the API payload and compute its expected checksum.
2. If no API row has that `externalId`, call `POST /api/words`, regardless of checksum. Add the returned `id` and checksum to the in-memory map.
3. If an API row exists and its checksum equals the expected checksum, skip the write.
4. If an API row exists and its checksum is null or differs, call `PUT /api/words/{id}` with the same `externalId`.

Omit the checksum from requests; the API computes and returns it. On an ambiguous POST timeout, re-read API pages and reconcile by `externalId` before retrying. The API has a unique index on non-null `externalId`, so blind POST retries can conflict. A process restart needs no local recovery file: the next run discovers completed creates from the API.

Use a confidential Keycloak client with the `client_credentials` grant for the API's `words#create` and `words#update` roles. Keep `KEYCLOAK_TOKEN_URL`, `KEYCLOAK_CLIENT_ID`, and `KEYCLOAK_CLIENT_SECRET` in environment configuration; do not place credentials or access tokens in the repository. The `JwtKeycloakContextHolder` checks cached JWT `exp` on every token request and obtains a new token 30 seconds before expiry.

The same service account also needs `examples#create` and `examples#update`. Read every page from `GET /api/examples?externalIdOnly=true`, which populates only `id`, non-null `externalId`, and `checksum`; other DTO fields are null, and linked words/translations are not loaded. For each active linked sentence, POST when its source ID is absent, skip when checksum matches, and PUT `/api/examples/{id}` when checksum differs. Update requests omit `externalId`; the API preserves it. On an ambiguous write, re-read the projected listing and reconcile by source ID before any further write. Each changed sentence uses an individual write request. HTTP 429 responses honor numeric `Retry-After` values or use capped backoff, up to four retries.

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

CSV export/download, CSV decoding, and API writes must fail visibly on unrecoverable HTTP or validation errors. Emit timestamped INFO logs for complete source table exports, API page progress, each word write outcome, each example outcome including disabled/unlinked skips, and final counts. Emit ERROR logs for failed writes and DEBUG logs for request starts and cached-token reuse. Report scanned, disabled-skipped, unlinked-skipped, created, updated, unchanged, and failed example counts alongside word counts. Do not convert unknown source types into `other`.

## Verification scope

Production code is checked with `cargo check` and the API's `./mvnw -DskipTests compile`. Do not run the full intake against the local API as part of implementation; it would write the full OpenRussian example dataset.
