# AWS — stack rules (starter snippet)

Common AWS footguns. Delete what doesn't apply.

> TODO(you): record your work setup — which services? IaC tool (CDK / Terraform /
> CloudFormation / SAM)? account + region structure? deploy pipeline?

- **Least-privilege IAM.** Scope policies to specific actions + resources; no
  wildcard `*` admin. Prefer roles over long-lived users.
- **Never hardcode credentials.** Use IAM roles / instance profiles / OIDC; take
  region + creds from the environment or the SDK default chain — never commit keys.
- **Secrets live in Secrets Manager / SSM Parameter Store**, not in code, env
  files, or task definitions in plaintext.
- **S3:** block public access by default; explicit bucket policies; encrypt at
  rest; versioning where the data matters. Don't make a bucket public to "make it
  work."
- **Infrastructure as code.** Change infra through IaC, not the console — console
  drift is invisible and unreproducible.
- **Tag resources** (owner/env/cost) and watch spend — right-size, set lifecycle
  policies, kill idle NAT gateways / instances.
- **Pin the region explicitly** and design for the AZ/region model of the service
  you're using.
