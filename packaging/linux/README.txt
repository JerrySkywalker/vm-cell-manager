VM Cell Manager portable GNU/Linux package
==========================================

This archive contains the repository-local x86_64-unknown-linux-gnu candidate.
Start with INSTALL.txt, then use `vmcell --help` and the project documentation:

  https://github.com/JerrySkywalker/vm-cell-manager/blob/dev/docs/linux-kvm-qga.md

QEMU is the provider, KVM is an accelerator, and QGA is the credentialless
guest transport for the initial prepared Linux QCOW2 path. Repository tests and
this package do not establish real KVM/QGA acceptance. The support matrix and a
dedicated-host receipt govern any future promotion.

This package does not install QEMU, change /dev/kvm permissions or groups, load
kernel modules, configure networking, start services, or mutate a VM.
