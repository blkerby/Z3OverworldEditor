# General guidelines

- If anything is unclear, please ask the user for clarification rather than
  guessing.
- Interpret the Ponytail approach as aiming to minimize the complexity of the
  project after applying a change, not minimizing the complexity of a change
  itself.
- Please apply the Ponytail approach to Markdown docs too: keep things as simple
  as possible.
- Ask for confirmation before diving into making changes, unless the exact
  desired code change is already clear from the user's request. This includes
  follow-up debugging after a reported regression: investigate and explain the
  likely cause first, but do not apply a speculative fix or design change until
  the user confirms that specific change.

# Rust style guidelines

- Unless the user specifies otherwise, any tests, asserts, and validations added
  to the code should be treated as temporary and removed before completing the
  task.
- Avoid iterator chains that combine operations such as mapping, filtering, and
  collecting. Prefer loops instead. Simple one-liners like
  `.into_iter().collect()` are ok.
- Avoid creating helper functions that are only used in one place or which would
  be simpler to inline.
- Likewise, avoid creating "wrapper" functions which do nothing other than
  call out to one other function.
- Avoid casts or `from`-conversions of `bool` to integer types. Use `if-else`
  instead.
- Function names should start with a verb.
