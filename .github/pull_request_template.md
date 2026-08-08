## Summary

Describe the change and the user-visible or architectural outcome.

## Scope

- [ ] Core model
- [ ] Provider
- [ ] Guest transport
- [ ] Image/state handling
- [ ] Documentation
- [ ] CI/tooling

## Safety and ownership

- Does this change perform host-global mutation?
- Can it affect foreign/non-vmcell VMs?
- Does it change image immutability or cell ownership semantics?

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-targets --all-features`

## Notes

Call out platform-specific validation that could not run in generic CI.
