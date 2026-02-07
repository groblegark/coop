# Shared libraries: wok issue tracking and git merge queue.

import "oj/wok" {
  alias = "wok"

  const "prefix" { value = "coop" }
  const "check"  { value = "make check" }
  const "submit" { value = "oj queue push merges --var branch=\"$branch\" --var title=\"$title\"" }
}

import "oj/git" {
  alias = "git"

  const "check" { value = "make check" }
}
