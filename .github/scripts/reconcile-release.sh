#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_OUTPUT:?GITHUB_OUTPUT must be set}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"
: "${GITHUB_SHA:?GITHUB_SHA must be set}"
: "${EXPECTED_RELEASE_SHA:?EXPECTED_RELEASE_SHA must be set}"

metadata="$(cargo metadata --no-deps --format-version 1)"
crate_name="$(jq -er '.packages[] | select(.name == "graphql-static-analysis") | .name' <<<"$metadata")"
crate_version="$(jq -er '.packages[] | select(.name == "graphql-static-analysis") | .version' <<<"$metadata")"
tag="v${crate_version}"

scratch_dir="$(mktemp -d)"
trap 'rm -rf "$scratch_dir"' EXIT

archive="$scratch_dir/${crate_name}-${crate_version}.crate"
archive_url="https://static.crates.io/crates/${crate_name}/${crate_name}-${crate_version}.crate"
http_status="$(
  curl --silent --show-error --location \
    --header "User-Agent: ${crate_name}-release-reconciler" \
    --output "$archive" \
    --write-out '%{http_code}' \
    "$archive_url"
)"

if [[ "$http_status" == "404" ]]; then
  {
    echo "complete=false"
    echo "published=false"
    echo "repaired=false"
  } >>"$GITHUB_OUTPUT"
  echo "${crate_name} ${crate_version} is not published; there is nothing to reconcile"
  exit 0
fi

if [[ "$http_status" != "200" ]]; then
  echo "crates.io returned HTTP ${http_status} for ${archive_url}" >&2
  exit 1
fi

published_sha="$(
  tar -xOf "$archive" "${crate_name}-${crate_version}/.cargo_vcs_info.json" |
    jq -er '.git.sha1'
)"

if [[ "$published_sha" != "$EXPECTED_RELEASE_SHA" ]]; then
  echo "crates.io contains commit ${published_sha}, expected ${EXPECTED_RELEASE_SHA}" >&2
  exit 1
fi

git cat-file -e "${published_sha}^{commit}"
if ! git merge-base --is-ancestor "$published_sha" "$GITHUB_SHA"; then
  echo "published commit ${published_sha} is not an ancestor of workflow commit ${GITHUB_SHA}" >&2
  exit 1
fi

echo "published=true" >>"$GITHUB_OUTPUT"
repaired=false

if tag_ref="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/tags/${tag}" 2>/dev/null)"; then
  tag_object_type="$(jq -er '.object.type' <<<"$tag_ref")"
  tag_object_sha="$(jq -er '.object.sha' <<<"$tag_ref")"
  if [[ "$tag_object_type" == "tag" ]]; then
    tag_target="$(
      gh api "repos/${GITHUB_REPOSITORY}/git/tags/${tag_object_sha}" --jq '.object.sha'
    )"
  elif [[ "$tag_object_type" == "commit" ]]; then
    tag_target="$tag_object_sha"
  else
    echo "tag ${tag} has unsupported target type ${tag_object_type}" >&2
    exit 1
  fi

  if [[ "$tag_target" != "$published_sha" ]]; then
    echo "tag ${tag} points to ${tag_target}, but crates.io contains ${published_sha}" >&2
    exit 1
  fi
else
  tag_object_sha="$(
    gh api --method POST "repos/${GITHUB_REPOSITORY}/git/tags" \
      --raw-field "tag=${tag}" \
      --raw-field "message=chore: Release package ${crate_name} version ${crate_version}" \
      --raw-field "object=${published_sha}" \
      --raw-field "type=commit" \
      --jq '.sha'
  )"
  gh api --method POST "repos/${GITHUB_REPOSITORY}/git/refs" \
    --raw-field "ref=refs/tags/${tag}" \
    --raw-field "sha=${tag_object_sha}" >/dev/null
  repaired=true
  echo "created missing tag ${tag} at published commit ${published_sha}"
fi

if gh release view "$tag" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
  echo "GitHub release ${tag} already exists"
else
  gh release create "$tag" \
    --repo "$GITHUB_REPOSITORY" \
    --verify-tag \
    --title "${crate_name} ${crate_version}" \
    --generate-notes
  repaired=true
  echo "created missing GitHub release ${tag}"
fi

{
  echo "repaired=${repaired}"
  echo "complete=true"
} >>"$GITHUB_OUTPUT"
