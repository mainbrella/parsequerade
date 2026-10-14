# Parsequerade

Parsequerade is a planned JSON REST facade for App Store Connect. It takes Apple's resource envelopes, type identifiers, and relationship links and presents a smaller API of plain objects and predictable resource URLs.

The goal is to let a client list apps, edit a localized description, update a review contact, or read sales data without signing Apple JWTs, looking up internal child IDs, or assembling relationship payloads.

**Status:** This repository documents the design. The service is not implemented yet; the facade routes and responses below are proposed contracts. See [rest.md](rest.md) for the implementation plan, supported fields, and rollout milestones.

## Why a facade?

Apple already provides a REST API that returns JSON. The difficulty is its resource and relationship model. For example, Apple's [List apps endpoint](https://developer.apple.com/documentation/appstoreconnectapi/get-v1-apps) wraps app fields in `data` and `attributes`, adds a resource `type`, and exposes related resources through separate links.

An app's metadata also spans several resources. App names and privacy URLs belong to app-info localizations. Descriptions and release notes belong to version localizations. A review contact is another resource associated with a version. Editing one field can require finding the right parent, following a relationship, resolving a child ID, and then constructing a typed update body.

Parsequerade moves that translation into one service. Clients work with app IDs, explicit app-info or version IDs, locale strings, and ordinary JSON fields.

## Before and after

These examples use fictional values and show only selected fields.

### List apps

Apple request:

```http
GET https://api.appstoreconnect.apple.com/v1/apps?filter[bundleId]=com.example.notes
```

Apple response shape:

```json
{
  "data": [
    {
      "type": "apps",
      "id": "1234567890",
      "attributes": {
        "name": "Example Notes",
        "bundleId": "com.example.notes",
        "primaryLocale": "en-US"
      },
      "relationships": {
        "appStoreVersions": {
          "links": {
            "related": "https://api.appstoreconnect.apple.com/v1/apps/1234567890/appStoreVersions"
          }
        }
      }
    }
  ]
}
```

Proposed Parsequerade request, relative to a configured connection:

```http
GET /apps?bundleId=com.example.notes
```

Proposed response:

```json
{
  "items": [
    {
      "id": "1234567890",
      "name": "Example Notes",
      "bundleId": "com.example.notes",
      "primaryLocale": "en-US"
    }
  ],
  "nextCursor": null
}
```

The client can read `response.items[0].name` directly. Single-resource reads return a plain object; collections use `items` and `nextCursor`.

### Edit a localized description

First, list versions with `GET /apps/{appId}/versions` and choose the intended version. Then address its locale directly:

```sh
BASE='https://asc.example.com/v1/connections/conn_example'

curl -X PATCH "$BASE/apps/1234567890/versions/version_123/localizations/en-US" \
  -H "Authorization: Bearer $ASC_FACADE_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: description-change-001' \
  -d '{"description":"Keep your notes organized.","supportUrl":"https://example.com/support"}'
```

`asc.example.com` is a placeholder for your future deployment. The service would resolve `en-US` to the Apple localization ID, check its ownership, and send Apple's required payload internally:

```json
{
  "data": {
    "type": "appStoreVersionLocalizations",
    "id": "localization_456",
    "attributes": {
      "description": "Keep your notes organized.",
      "supportUrl": "https://example.com/support"
    }
  }
}
```

Clients would receive the updated flat localization object. A missing locale returns `404 localization_not_found`; creating a locale is an explicit `POST`.

## The proposed API

Each Apple connection has a base URL:

```text
https://asc.example.com/v1/connections/{connectionId}
```

Requests use a facade API key. The service owns the configured Apple credentials and JWT lifecycle, and checks the caller's permissions for each connection and resource.

Routes below are relative to that base. This is a summary; [rest.md](rest.md#resource-routes) defines the full planned contract and delivery stages.

| Task | Proposed route |
| --- | --- |
| List or read apps | `GET /apps`, `GET /apps/{appId}` |
| Select app information | `GET /apps/{appId}/app-infos` |
| Read or edit an app-level locale | `GET`, `PATCH /apps/{appId}/app-infos/{appInfoId}/localizations/{locale}` |
| Select a version | `GET /apps/{appId}/versions?platform=IOS&state=PREPARE_FOR_SUBMISSION` |
| Read or edit a version-level locale | `GET`, `PATCH /apps/{appId}/versions/{versionId}/localizations/{locale}` |
| Read, create, or edit a review contact | `GET`, `POST`, `PATCH /apps/{appId}/versions/{versionId}/review-contact` |
| Read sales as JSON | `GET /sales-reports?frequency=monthly&period=2026-09&vendorNumber=12345678` |
| Preview a metadata copy | `POST /copy-plans` |
| Execute a previewed copy | `POST /copy-plans/{planId}/executions` |
| Inspect workflow progress | `GET /operations/{operationId}` |

Writes start with the specific fields listed in the plan. Reading a field does not make it writable. Uploads, submission, pricing, and broader TestFlight management need separate contract work.

## What the service handles

- **Resource translation.** Flatten `attributes`, build Apple's typed request envelopes, and translate supported relationship edits. For example, `{"primaryCategoryId":"UTILITIES"}` becomes a category relationship update.
- **Target selection.** Resolve localization and review-contact IDs internally. Require explicit app-info and version targets for writes; never silently pick the first version or substitute another locale.
- **Pagination.** Return bounded pages with opaque cursors. Clients pass `cursor` back to the facade instead of following Apple pagination URLs. Incomplete upstream traversal is an error.
- **Updates and errors.** Omitted PATCH fields stay unchanged. Clearing with `null` is allowed only for supported fields. Preserve valid empty strings, `false`, and `0`; reject unsupported fields. Return stable error codes and field-level details.
- **Sales decoding.** Download and decode Apple's compressed tabular reports into typed JSON rows. Preserve monetary precision as decimal strings and keep currencies separate. An unavailable report is an error, rather than zero sales.
- **Write tracking.** Require idempotency keys and record write attempts. If a dispatched write times out, expose its uncertain outcome for reconciliation instead of blindly repeating it.
- **Copy workflows.** Preview selected metadata changes before execution, then expose progress and per-step outcomes. Multi-request copies can partially succeed; cancellation does not undo completed writes.

Apple's permissions, validation rules, and editable states still apply. The facade gives clients a consistent way to address resources and inspect outcomes while keeping Apple's authentication, graph traversal, and report parsing inside the service.
