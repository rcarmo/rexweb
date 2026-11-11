# Releasing rexweb

Release archives target macOS on Apple Silicon and Intel. Both include the Go executable, built frontend, documentation and third-party notices. They are unsigned and are not notarised.

## Prepare

1. Update `VERSION` and the corresponding entry in `CHANGELOG.md`.
2. Install locked dependencies with `make install-dev`.
3. Run `make check race smoke release` on macOS. The package test extracts the native archive and checks its version, assets, default session and HTTP endpoint.
4. Review `dist/SHA256SUMS`, the archive contents and licence notices. Keep generated archives, `bin/`, `web/node_modules/` and `web/dist/` out of Git.

The CI workflow runs checks, race detection and sandbox smoke tests on macOS ARM64 and Intel. The release workflow requires those jobs to pass before packaging.

## GitHub setup

Create the GitHub repository, add it as this checkout's `origin`, and push the `main` branch. Enable private vulnerability reporting in the repository's security settings. Set an appropriate repository description and review Actions permissions.

The release job needs the workflow token's `contents: write` permission to create a draft and attach assets. Other jobs use read-only repository permissions. No signing credentials or long-lived publishing token are required.

## Create a draft

Commit the release preparation, then tag that exact commit:

```sh
version=$(cat VERSION)
git tag -a "v$version" -m "rexweb $version"
git push origin "v$version"
```

Pushing a `v*` tag starts `.github/workflows/release.yml`. The workflow checks that the tag matches `VERSION`, runs CI, builds both archives and creates a **draft** GitHub release. The initial `0.1.0-preview.1` draft is marked as a prerelease. Preview, alpha, beta and release-candidate version strings are marked as prereleases automatically.

Review the generated notes and assets in GitHub, then publish the draft when ready. If a tag already has a release, the workflow fails instead of replacing its assets. Fix the problem and use a new version for an already published release.
