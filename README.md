# srvcs-floatmultiply

Floating-point multiplication for srvcs.cloud: computes `a * b` and returns the
product as an `f64` (a JSON number that may have a fractional part). Unlike the
integer arithmetic leaves, this is a **float** service — both integer and
fractional operands are accepted, and the result is a 64-bit float.

## Concern

`float arithmetic: a * b`

## Dependencies

- `srvcs-isnumber` — the single source of truth for "is this a number". Each
  operand is validated over HTTP before the multiplication is performed.

## API

### `GET /`

Service identity.

```json
{
  "service": "srvcs-floatmultiply",
  "concern": "float arithmetic: a * b",
  "depends_on": ["srvcs-isnumber"]
}
```

### `POST /`

Request:

```json
{ "a": 2.5, "b": 4 }
```

Response `200`:

```json
{ "a": 2.5, "b": 4, "result": 10.0 }
```

`result` is an `f64`. Because floating-point multiplication is not exact,
clients should compare results approximately.

Responses:

- `200` — `{ "a", "b", "result": <f64> }`
- `422` — an operand is not a number (forwarded from / decided by
  `srvcs-isnumber`)
- `503` — `srvcs-isnumber` is unreachable; the service reports itself degraded
  rather than guessing.

## Configuration

| Env var              | Default                  | Description                       |
| -------------------- | ------------------------ | --------------------------------- |
| `SRVCS_BIND_ADDR`    | `0.0.0.0:8080`           | Host:port to bind.                |
| `SRVCS_ISNUMBER_URL` | `http://127.0.0.1:8081`  | Base URL of `srvcs-isnumber`.     |
| `RUST_LOG`           | `info,tower_http=info`   | Log filter.                       |
| `SRVCS_ENV`          | `development`            | Environment label.                |

## Standard endpoints

`/healthz`, `/readyz`, `/metrics`, `/openapi.json`.
