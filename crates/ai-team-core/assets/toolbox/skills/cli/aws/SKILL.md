---
name: aws-cli
description: Operate AWS via the aws CLI — inspect resources, logs, S3, secrets (read-first). Use for AWS tasks instead of a fleet of AWS MCP servers (which AWS itself warns can confuse agents).
---

# aws — CLI recipes

- **Identity/region:** `aws sts get-caller-identity` — always know the region +
  profile (`--profile`, `--region`); never assume them.
- **Read first:** prefer `describe` / `list` / `get` before any mutation; confirm
  before create / delete / put.
- **S3:** `aws s3 ls s3://<bucket>` — respect the public-access-block; never make a
  bucket public to "make it work".
- **Logs:** `aws logs tail <group> --follow`
- **Secrets:** `aws secretsmanager get-secret-value …` / SSM — never print secret
  values into the transcript.

Safety: least-privilege. Mutations are real and can be costly or irreversible —
surface the exact command + target and get an explicit go for anything that
creates, deletes, or modifies infra. Prefer IaC over ad-hoc CLI mutations.
