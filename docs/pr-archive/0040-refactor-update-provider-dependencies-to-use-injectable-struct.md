# #40 — refactor: update provider dependencies to use Injectable struct

Merged 2026-04-18 into `master` from `refactor/injectable-dep-type`, commit [`62df2b7`](https://github.com/ulo-rs/ulo/commit/62df2b7b62795efbf9ab406fdda3afc94ae07156).

Introduce the `Injectable` struct to encapsulate provider dependencies, enhancing readability and simplifying the build method signatures across various provider factories. This change improves the clarity of the code without altering existing functionality.
