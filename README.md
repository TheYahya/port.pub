# port.pub
Share your local http server with public internet.

### How to use
1. [Install rust](https://www.rust-lang.org/tools/install)
2. Clone this repository: `git clone https://github.com/TheYahya/port.pub.git`
3. Move to project directory: `cd port.pub`
4. Run: `cargo run -p cli -- http --port {PORT}` (replate `{PORT}` with your local http server port).

   e.g. `cargo run -p cli -- http --port 8081`
5. You'll see something like following:
```
port: 8081
port published at: {uuid}.port.pub
```
6. You can access your local project at: `{uuid}.port.pub`

