# Releasing filc

Crates.io releases are permanent and cannot be overwritten. Release only an
exact revision that is durably pushed and passes both declared Fabric
operations:

```console
fabric check --no-push
fabric package --no-push
```

Before the first release, add an authoritative public `repository` URL to all
three package manifests. Add a project `homepage` as well if one exists. Do not
use the private Fabric endpoint or invent a public URL.

The crates must be published in dependency order:

```console
cargo publish --locked -p filc-macros
cargo publish --locked -p filc-build
```

Wait until `filc-macros` is visible in the crates.io index, then verify and
publish the main crate without the local verification patch:

```console
cargo publish --locked --dry-run -p filc
cargo publish --locked -p filc
```

Confirm all three registry pages and docs.rs builds, then tag the exact release
revision. Never place a crates.io token in this repository or its logs.
