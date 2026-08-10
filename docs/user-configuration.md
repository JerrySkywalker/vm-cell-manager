# User configuration

VM Cell Manager accepts a small schema-versioned JSON file for non-authorizing
defaults. Configuration is optional. It never replaces the installation,
runtime, ownership, provider-identity, or mutation-lock proofs required by the
engine.

```json
{
  "schema_version": 1,
  "defaults": {
    "state_root": "state",
    "provider": "hyperv",
    "cpu_count": 2,
    "memory_mib": 4096,
    "lock_timeout_ms": 0,
    "readiness_timeout_seconds": 120,
    "action_timeout_seconds": 300,
    "human_output": "normal"
  }
}
```

The config-file selection order is:

1. the exact file named by global `--config PATH`;
2. `config.json` in the platform application config directory selected by
   `directories::ProjectDirs` for VM Cell Manager;
3. built-in defaults when that implicit file does not exist.

There is deliberately no environment-variable config override. A missing
explicit file is an error. A missing implicit file is not. If the platform
cannot provide an application config directory, vmcell uses built-in defaults;
it never falls back to an ambient working-directory config file.

For each allowed setting, the command line wins over the selected config, and
the config wins over the built-in default. Persisted provider identity still
wins for commands acting on an existing image or cell. The configured provider
is consulted when a new image validation/registration or `create` request
omits `--provider`. For `run`, an explicitly present configured provider is a
preference between CLI overrides and deterministic compatible native/default
selection; an absent provider setting does not invent a Hyper-V preference.
An unavailable preferred path fails instead of silently falling through.

`state_root` may be absolute or relative to the directory containing the
config file. Dot segments are rejected. The normal state-store ordinary-path,
private-owner, reparse/symlink, identity, and lock gates still apply before any
state or provider mutation. `human_output: "quiet"` suppresses normal
`vmcell run` lifecycle progress; it does not suppress bounded guest output,
errors, or machine-readable output.

The file is bounded to 64 KiB, must be an ordinary non-reparse file, and is
opened read-only/no-follow. On Unix it must be owned by the effective user with
no group/other permission bits. Unknown fields, malformed JSON, unsafe paths,
out-of-range values, and unsupported schema versions fail before state or
provider access. JSON error output uses stable `vmcell.config.*` codes without
echoing file contents or paths.

The schema intentionally has no credential, guest command, accelerator,
`allow_tcg`, lifecycle, cleanup, ownership, installation, or provider-object
fields. Unknown fields are rejected. In particular, configuration cannot
silently enable TCG, grant destructive authority, adopt a VM, weaken ownership
checks, or supply a guest password.

Supported value bounds are:

- CPU count: 1 through 64;
- memory: 512 through 1,048,576 MiB;
- lock wait: 0 through 30,000 ms per acquisition;
- readiness and action timeouts: 1 through 3,600 seconds.

The schema is additive within version 1. Removing or reinterpreting a field
requires a new schema version. Unsupported versions fail closed; vmcell never
silently reinterprets them.
