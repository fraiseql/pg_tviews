# Write-path benchmark (#224)

FraiseQL-shaped TVIEWs (velocitybench's `tv_user`, `tv_post`, `tv_comment`, with the
joined entity nested as `author` / `post`) and five writes:

| write | touches |
|---|---|
| `bio` | 1 `tv_user` row, 10 `tv_post` rows (`author.bio`) |
| `username` | 1 `tv_user`, 10 `tv_post` (`author.username`), 50 `tv_comment` (`author.username`) |
| `title` | 1 `tv_post`, ~5 `tv_comment` (`post.title`) |
| `comment_insert` | 1 new `tv_comment` row |
| `bio_bulk100` | 100 `tv_user`, 1,000 `tv_post` rows in one statement |

`run.sh [seconds] [clients] [write ...]` prints each write's tps with pg_tviews, the
same write with `session_replication_role = replica` (no triggers), and the ratio.

Time a release build on a stock server only: the pgrx development build is
debug + cassert and several times slower.
