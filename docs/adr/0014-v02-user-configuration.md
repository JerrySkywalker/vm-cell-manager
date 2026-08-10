# ADR 0014: Versioned non-authorizing user configuration

## Status

Accepted for the v0.2 repository-local train.

## Decision

VM Cell Manager loads at most one bounded schema-versioned JSON configuration
file before opening state or dispatching to a provider. An explicit
`--config PATH` has precedence over the platform application config path.
Command-line fields have precedence over config fields, which have precedence
over built-in defaults.

Version 1 permits only state-root preference, default provider for new work,
CPU and memory defaults, bounded lock/readiness/action timeouts, and normal or
quiet human run progress. Existing durable image/cell provider identity is not
overridden by config.

The config is data, not authority. It cannot carry credentials, commands,
accelerator or TCG permission, lifecycle intent, cleanup policy, ownership,
installation identity, provider object identity, or exceptions to state and
provider proof. Unknown fields and unsupported versions fail closed before
state/provider access.

Files are size-bounded and opened as ordinary non-reparse inputs. Unix files
must be current-user private; relative state roots are anchored to the config
directory and reject dot segments. StateStore remains responsible for the
stronger authority and containment proof when a resolved root is used.

## Consequences

Automation can continue to pass every value explicitly. Human users can keep
safe repetitive defaults without placing secrets on argv or weakening the
engine. Missing implicit config preserves legacy v0.1 defaults. Malformed or
unsupported config has stable redacted error taxonomy and performs no state or
provider mutation.
