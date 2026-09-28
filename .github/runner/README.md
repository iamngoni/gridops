# GridOps CI runner

GridOps' own CI uses an isolated runner image with Node.js 22.13 and Rust 1.96 already installed. The runtime runner remains capability-free with `no-new-privileges`; build tooling is installed while the image is built, never by a privileged workflow step.

Build the image on the GridOps Docker host:

```sh
docker build \
  --file .github/runner/Dockerfile \
  --tag ghcr.io/iamngoni/gridops-ci-runner:node22-rust196 \
  .github/runner
```

Push the image to GHCR, then configure the pool with that image and the `gridops-ci` label:

```sh
docker push ghcr.io/iamngoni/gridops-ci-runner:node22-rust196
```

The CI workflow requests the `gridops-ci` label so jobs cannot land on a generic runner without the required toolchain.
