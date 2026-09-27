# Russian Cheatsheet Intake

Date: 2026-09-27

## Goal

Create a Rust CLI that synchronizes active OpenRussian words into the Russian Cheatsheet API. Each run scans the source tables and uses the API checksum to skip unchanged API writes. No local sync database.

## Scope

- Import `words.bare`, source word type, verb aspect, English translations, and bare word forms.
- Read public OpenRussian data through TogetherDB; no PostgreSQL/MySQL DSN required.
- Create missing API words and update existing imported words when their canonical checksum differs.
- Skip disabled OpenRussian rows. Never delete API words.
- Do not generate Spanish translations in this version.
- Do not import examples, relations, usage text, or audio URLs in this version.
- Preserve OpenRussian attribution and CC BY-SA licensing information in project documentation.

## Source access and mapping

The source adapter resolves the worker from the public connection ID in the supplied TogetherDB URL, then pages through its worker REST endpoints using `start` and `count`. This is the REST interface used by TogetherDB's web client; it currently returns public rows without credentials, but TogetherDB does not document it as a supported third-party client API.

Read these tables:

| Source | Mapping |
| --- | --- |
| `words` | `id` → external ID string; `bare` → API `russian`; `type` → API `type`; import only `disabled = false` |
| `words_forms` | `word_id`, `form_type`, `position`, `form_bare` → API `forms` |
| `translations` | rows with `lang = 'en'`; `tl` and source `position` → English API translations |
| `verbs` | `word_id` and `aspect` → API `aspect`; required for `type = 'verb'` |

Use bare source text throughout: `words.bare` and `words_forms.form_bare`, never `accented` or `form`. Preserve text exactly; do not trim or normalize Unicode. If multiple form rows share a `form_type`, sort by `position` and join `form_bare` values with `; ` because API form values are single strings. If a word has no form rows, send `forms: null`; when forms exist, send the full API forms object with absent fields set to null.

Send an empty translations array when a word has no English translations. Keep source positions. Reject unknown word types or form types instead of silently mapping or dropping them.

## API synchronization

Fetch every page from `GET /api/words` and build an in-memory map keyed by `externalId`, storing API `id` and `checksum`. `externalId` is the OpenRussian `words.id` and is the identity key; Russian text and checksum are not identity keys. This handles homographs and identical payloads with distinct OpenRussian IDs.

For each active source word:

1. Normalize the API payload and compute its expected checksum.
2. If no API row has that `externalId`, call `POST /api/words`, regardless of checksum. Add the returned `id` and checksum to the in-memory map.
3. If an API row exists and its checksum equals the expected checksum, skip the write.
4. If an API row exists and its checksum is null or differs, call `PUT /api/words/{id}` with the same `externalId`.

Omit the checksum from requests; the API computes and returns it. On an ambiguous POST timeout, re-read API pages and reconcile by `externalId` before retrying. The API has a unique index on non-null `externalId`, so blind POST retries can conflict. A process restart needs no local recovery file: the next run discovers completed creates from the API.

Use the existing bearer token from environment configuration for the API's `words#create` and `words#update` roles. Do not place tokens in the repository.

## Checksum contract

The Rust implementation must match `WordChecksumService` exactly. Its canonical word includes `russian`, `type`, imported translations, `usage`, `audioUrl`, `aspect`, `forms`, and outgoing relations. It serializes compact UTF-8 JSON with sorted property names and null values included, then calculates lowercase SHA-256 hex. Imported translations sort by language, position, then text. The hash does not include `externalId`, API UUID, audit timestamps, publication state, or the checksum itself.

For this version, the expected API projection is:

- `russian`: `words.bare`
- `type`: source `words.type`
- `translations`: English source rows only
- `aspect`: `verbs.aspect` for verbs; null otherwise
- `forms`: mapped bare forms, or null if none
- `usage`: null
- `audioUrl`: null
- `relations`: empty list

Manual translations are preserved by the API and excluded from its checksum. The importer assumes OpenRussian records it manages have null usage/audio and no outgoing API relations. If another writer populates those fields, the full API checksum can remain different from this importer projection; an update can also clear usage/audio. Before such records are in scope, add an API checksum specifically for importer-managed fields or expand the importer to own those fields. Do not clear relations to force a checksum match.

## Error handling and reporting

Page reads and API writes must fail visibly on unrecoverable HTTP, decoding, or validation errors. Report counts for source words scanned, disabled words skipped, created, updated, unchanged, and failed. Do not convert unknown source types into `other`.

## Verification scope

No automated tests or test commands are included in this design; user has not requested testing or verification.
