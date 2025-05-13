# port.pub
Share your local http server with public internet.

### How to use
1. [Install rust](https://www.rust-lang.org/tools/install)
2. Install `portpub` CLI: `cargo install --git https://github.com/TheYahya/port.pub portpub`
3. Run: `portpub http --port {PORT}` (replate `{PORT}` with your local http server port).

   e.g. `portpub -- http --port 8081`
4. You'll see something like following:
```
port: 8081
port published at: {uuid}.port.pub
```
5. You can access your local project at: `{uuid}.port.pub`

