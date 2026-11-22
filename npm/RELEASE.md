# Publishing Codeidx on npm

The first npm version is `0.1.0`. Build from the release commit on main. Do not repoint the existing Git tag.

1. Run the **Build npm release artifacts** workflow from the chosen commit. Download both artifacts and verify all three tarballs. This build does not publish and does not need an npm credential.
2. Sign in to npm using your normal secure login method. Check `npm whoami` and verify write access to the `@thatgfsj` scope. When using a token, keep it outside the repository and out of shell arguments.
3. Publish the platform packages first, then the launcher. Do not publish the launcher if either native package failed verification or publication.

```sh
npm publish thatgfsj-codeidx-linux-x64-0.1.0.tgz --access public
npm publish thatgfsj-codeidx-win32-x64-0.1.0.tgz --access public
npm publish thatgfsj-codeidx-0.1.0.tgz --access public
```

Check the public version with `npm view @thatgfsj/codeidx@0.1.0`. On each platform, install with optional dependencies enabled and verify both commands. Linux packages are built against Ubuntu 24.04 glibc (2.39); Windows packages target x64 MSVC. For other hosts, build from source. For MCP, configure the installed executable as `codeidx-mcp`, with no arguments.
