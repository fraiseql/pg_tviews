#!/bin/bash
set -euo pipefail

# Semantic version bump script for pg_tviews
# Usage: ./bump-version.sh major|minor|patch|prerelease [--dry-run]

if [ $# -lt 1 ]; then
    echo "Usage: $0 major|minor|patch|prerelease|release [--dry-run]"
    echo ""
    echo "Examples:"
    echo "  $0 minor          # 0.1.0 → 0.2.0"
    echo "  $0 patch          # 0.1.0 → 0.1.1"
    echo "  $0 prerelease     # 0.2.0 → 0.2.0-rc.1"
    echo "  $0 release        # 0.2.0-rc.1 → 0.2.0"
    exit 1
fi

BUMP_TYPE=$1
DRY_RUN=${2:-}

# Parse current version
CURRENT_VERSION=$(grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/')
echo "Current version: $CURRENT_VERSION"

# Function to compare versions
semver_bump() {
    local version=$1
    local bump_type=$2

    # Remove prerelease suffix
    base_version=$(echo "$version" | sed 's/-.*$//')

    # Parse components
    major=$(echo "$base_version" | cut -d. -f1)
    minor=$(echo "$base_version" | cut -d. -f2)
    patch=$(echo "$base_version" | cut -d. -f3)

    case "$bump_type" in
        major)
            echo "$((major + 1)).0.0"
            ;;
        minor)
            echo "$major.$((minor + 1)).0"
            ;;
        patch)
            echo "$major.$minor.$((patch + 1))"
            ;;
        prerelease)
            echo "$major.$minor.$patch-rc.1"
            ;;
        release)
            # Remove prerelease suffix
            echo "$base_version"
            ;;
        *)
            echo "Unknown bump type: $bump_type" >&2
            exit 1
            ;;
    esac
}

NEW_VERSION=$(semver_bump "$CURRENT_VERSION" "$BUMP_TYPE")
echo "New version: $NEW_VERSION"

if [ -z "$DRY_RUN" ]; then
    # Update Cargo.toml
    sed -i.bak "s/^version = .*/version = \"$NEW_VERSION\"/" Cargo.toml
    rm Cargo.toml.bak

    # Update lock file
    cargo update --offline || true

    # Create commit
    git add Cargo.toml Cargo.lock
    git commit -m "chore: Bump version to $NEW_VERSION"

    # Create tag if not prerelease
    if [[ "$NEW_VERSION" != *"-"* ]]; then
        git tag "v$NEW_VERSION"
        echo "✅ Tag created: v$NEW_VERSION"
    fi

    echo "✅ Version bumped to $NEW_VERSION"
    echo "Next: Push with: git push origin main --tags"
else
    echo "🔍 Dry run (no changes made)"
fi