# GridOps CI runner

GridOps' own CI uses an isolated runner image with Node.js 22.13 and Rust 1.96 already installed. The runtime runner remains capability-free with `no-new-privileges`; build tooling is installed while the image is built, never by a privileged workflow step.

Build the image on the GridOps Docker host:

```sh
docker build \
  --file .github/runner/Dockerfile \
  --tag gridops-ci-runner:node22-rust196 \
  .github/runner
```

Configure the pool with that image and the `gridops-ci` label. The CI workflow requests that label so jobs cannot land on a generic runner without the required toolchain.
