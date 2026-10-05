# Russian Cheatsheet Intake

Rust CLI that syncs active OpenRussian words to Russian Cheatsheet API. It loads source words, English translations, verb aspects, audio URLs, English usage, and forms; then creates missing words and updates words whose API checksum differs. Disabled words are skipped. Intake never deletes API words.

Intake maps `words.audio` to `audioUrl`, `words.usage_en` to `usage`, and `words_forms.form_bare` to source-named API forms. Repeated forms with the same `form_type` are sorted by source position and row ID, then joined with `; `. Word translation objects contain `language`, `text`, and `position`; `managedBy` is internal to the API. Blank types and aspects stay null; unsupported types and form types fail visibly. Manual API translations remain API-managed.

## Run

Copy `.env.example` to `.env`, then set the API URL and Keycloak client details. Keycloak client must be confidential, have service accounts enabled, and its service account needs `words#create` and `words#update` roles.

```sh
cp .env.example .env
# Fill RUSSIAN_CHEATSHEET_API_URL, KEYCLOAK_TOKEN_URL, KEYCLOAK_CLIENT_ID, KEYCLOAK_CLIENT_SECRET
cargo run --release
```

Logs include UTC timestamps and INFO events for complete source CSV exports, API page progress, and each create, update, or unchanged word. Intake downloads and parses each full CSV in memory without row chunks. Set `RUST_LOG=debug` before `cargo run --release` to see request starts and cached-token reuse.

Set `KEYCLOAK_TOKEN_URL` to the full OpenID Connect token endpoint URL, usually `https://<host>/realms/<realm>/protocol/openid-connect/token`. Intake uses the `client_credentials` grant. `JwtKeycloakContextHolder.get_token()` checks expiry on every request and renews the token 30 seconds early, using the earlier of JWT `exp` and Keycloak `expires_in`.

The process environment overrides values from `.env`. Set `OPENRUSSIAN_TOGETHERDB_URL` to override the default public OpenRussian database link. The link must use the TogetherDB database path format `/db/{connectionId}/{databaseName}/{tableName}`.

The source uses TogetherDB's public worker export endpoints: one complete CSV export per table, followed by download using its export key. These endpoints serve TogetherDB's web client and are not documented as a stable third-party API, so a TogetherDB change may require updating the source adapter.

## OpenRussian data

OpenRussian dictionary data is licensed CC BY-SA 4.0. Source: [OpenRussian dictionary data](https://en.openrussian.org/dictionary-data). Keep this attribution with redistributed data and publish modifications under the same license.
