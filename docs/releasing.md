# Releasing Probatio

The program is the Rust binary. The crate version is 0.7.1. `README.md` and `AGENTS.md` are the contract. Changelog entries through 0.3.0 describe the TypeScript program.

A release needs a green `cargo test` on Linux, macOS, and Windows, and a reason in the CHANGELOG. The version string is the same in `Cargo.toml`, `Cargo.lock`, `server.json`, the npm `package.json` files, the README, and the CHANGELOG. The MCP server reports `CARGO_PKG_VERSION`. `probatio --version` prints that same version.

1. Land the version commit on `main`.
2. Wait until `test` and `self-score` are green on that commit.
3. Tag that commit and push the tag: `git tag -a v<version> -m v<version> && git push origin v<version>`.
4. `.github/workflows/release.yml` builds `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-pc-windows-msvc`, then attaches those binaries to the GitHub release.
5. The same workflow publishes `probatio-linux-x64`, `probatio-darwin-arm64`, `probatio-darwin-x64`, and `@iron_adamant/probatio-win32-x64`, then `probatio`. The Windows package is scoped because npm refused the unscoped name. It is published with `--access public`. The workflow uses `NPM_TOKEN` (`secrets.NPM_TOKEN`) and passes `--provenance` when this npm supports that flag. If `NPM_TOKEN` is not set, the job logs `NPM_TOKEN is not set. Skipping npm publish.` and leaves the GitHub release in place. A package version that is already on npm is skipped, so a failed publish can be run again.
6. To publish an existing tag without rebuilding, run the `npm publish` workflow from `main` and pass the tag (`v0.5.0`). It downloads that GitHub release and runs `scripts/npm-publish.sh`. It does not build binaries and it does not create another GitHub release.
7. After npm, if the repository variable `MCP_REGISTRY_PUBLISH` is `true`, `release.yml` runs `.github/workflows/mcp-registry.yml`, which publishes the tag's `server.json` to the MCP Registry as `io.github.IronAdamant/probatio`. It checks that npm has that version with the same `mcpName`, installs `mcp-publisher` 1.8.7 from the registry's GitHub release, and logs in with GitHub Actions OIDC (`mcp-publisher login github-oidc`), so it needs no secret. To list a tag that is already on npm, run the `mcp registry` workflow from `main` with that tag.
8. A later commit can record the publish date and gitHead. The tag stays on the version commit.

`scripts/npm-pack-local.sh` builds tarballs for a local install check. It does not publish. The `@probatio` npm scope belongs to a different project. Linux and macOS packages stay unscoped. The Windows package is `@iron_adamant/probatio-win32-x64`.

Do not publish from a laptop, and do not publish a version that has no CHANGELOG entry.

To cut a tag and a GitHub release without publishing to npm, set the repository variable `NPM_PUBLISH` to `false` before tagging, then run the `npm publish` workflow for that tag later (and remove the variable).
