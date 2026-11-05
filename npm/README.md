# probatio

Version 0.7.0 installs the Rust binary. The `probatio` command is a small Node launcher. npm installs one platform package next to it and the launcher runs that binary.

`probatio mcp` starts the MCP server on stdin. A client runs `npx -y probatio mcp`. The registry name is `io.github.IronAdamant/probatio`.

| Package | Machine |
| --- | --- |
| `probatio-linux-x64` | Linux x64, musl build |
| `probatio-darwin-arm64` | macOS Apple silicon |
| `probatio-darwin-x64` | macOS Intel |
| `@iron_adamant/probatio-win32-x64` | Windows x64 |

```bash
npm install probatio
probatio --version
```

The command prints one JSON object. `summary` is the crate version. A `v*` tag on [IronAdamant/Probatio](https://github.com/IronAdamant/Probatio) publishes the GitHub release and these packages.
