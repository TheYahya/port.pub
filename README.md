[![Crates.io](https://img.shields.io/crates/v/portpub.svg)](https://crates.io/crates/portpub)
[![Codecov](https://codecov.io/github/theyahya/port.pub/coverage.svg?branch=main)](https://codecov.io/gh/theyahya/port.pub)
[![Dependency status](https://deps.rs/repo/github/theyahya/port.pub/status.svg)](https://deps.rs/repo/github/theyahya/port.pub)

# port.pub
Share your local http server with public internet.

<img alt="port.pub demo" src="https://github.com/theyahya/port.pub/blob/main/demo/demo.gif" width="600" />

### How to use
1. [Install rust](https://www.rust-lang.org/tools/install)
2. Install `portpub` CLI: `cargo install portpub`
3. Run: `portpub http --port {PORT}` (replate `{PORT}` with your local http server port).

   e.g. `portpub -- http --port 8081`
4. You'll see something like following:
```
port: 8081
port published at: {uuid}.port.pub
```
5. You can access your local project at: `{uuid}.port.pub`

