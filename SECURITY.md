# Private files and security

Keys and GameShare passphrases are required for connection, but not for building or running unit tests.
Keep them outside the repository or in ignored `private/` / `secrets/` directories.
Do not include them in commits, release archives, issue attachments, screenshots or logs.

Settings store file paths, not key contents. The passphrase extractor creates a secret file under
`private/` beside settings; see [extraction instructions](docs/PASSPHRASE.md).
Keys and passphrase files are not encrypted by the application and are read into memory while in use.

## Sharing diagnostics

Settings and logs can contain personal paths and network information. Redact these before sharing.
Do not upload raw captures, console backups, NCA/NSO files, keys or passphrases.
Report unsupported extraction builds using the Build ID and redacted error text.

Host-signature verification is not implemented. Connect only to a trusted local host.
Never include secret material in a public vulnerability report.

## Before redistributing

Review the files being shared and run the [release checks](docs/RELEASING.md).
Ignore rules can be overridden, and the audit cannot detect every possible secret or encoding.
If a secret entered Git history, removing the current file alone does not remove it from that history.
