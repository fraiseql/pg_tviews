# Release Process for pg_tviews

## Pre-Release Checklist (2 weeks before)

### Code Preparation
- [ ] All features merged to main
- [ ] All tests passing locally
- [ ] Code review completed
- [ ] No open blockers

### Documentation
- [ ] README.md updated for new features
- [ ] API documentation up to date
- [ ] Migration guides created (if breaking changes)
- [ ] CHANGELOG.md drafted

### Quality Gates
- [ ] All tests pass: `cargo pgrx test --all`
- [ ] No clippy warnings: `cargo clippy --all-targets -- -D warnings`
- [ ] Documentation builds: `cargo doc --no-deps`
- [ ] Version bumped in Cargo.toml

## Release Candidate Steps

1. Create release branch:
```bash
git checkout -b release/v1.2.0 main
```

2. Update version:
```bash
./scripts/bump-version.sh minor
# Changes 1.1.0 → 1.2.0-rc.1
```

3. Update CHANGELOG.md with version header:
```
## [1.2.0-rc.1] - YYYY-MM-DD

### Added
...
```

4. Commit and tag:
```bash
git commit -am "chore: Prepare v1.2.0-rc.1"
git tag v1.2.0-rc.1
git push origin release/v1.2.0 --tags
```

5. Run full test suite on target versions:
```bash
cargo pgrx test --all
```

6. Address any issues and create RC.2, RC.3, etc. as needed

## Release Day (When RC is Stable)

### Final Steps

1. Update version to final (remove -rc suffix):
```bash
./scripts/bump-version.sh release
# Changes 1.2.0-rc.5 → 1.2.0
```

2. Update CHANGELOG.md:
```
## [1.2.0] - 2025-12-13  # ← Set actual date
```

3. Create final commit:
```bash
git commit -am "chore: Release v1.2.0"
git tag v1.2.0
git push origin release/v1.2.0 --tags
```

4. Create GitHub Release:
- Copy CHANGELOG.md section
- Add download links
- Mark as pre-release if RC
- Mark as latest release if final

5. Publish binaries and artifacts

### Post-Release

1. Delete release branch:
```bash
git branch -d release/v1.2.0
git push origin --delete release/v1.2.0
```

2. Update development version:
```bash
./scripts/bump-version.sh minor
# Start development for next version
git commit -am "chore: Start development for v1.3.0-dev"
```

3. Announce release:
- GitHub release page
- Community forums
- Social media
- Email newsletter (if applicable)

4. Monitor for issues:
- Watch bug reports
- Prepare patch releases if needed