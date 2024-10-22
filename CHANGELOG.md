# Changelog

# 0.1.9000

## 2024-10

- The document symbol kind for assigned variables is now `VARIABLE` (@kv9898, posit-dev/positron#5071). This produces a clearer icon in the outline.

- Added partial support for outline headers in comments (@kv9898, posit-dev/positron#3822).

- Sending long inputs of more than 4096 bytes no longer fails (posit-dev/positron#4745).

- Jupyter: Fixed a bug in the kernel-info reply where the `pygments_lexer` field
  would be set incorrectly to `""` (#553).

  Following this fix, syntax highlighting now works correctly in Jupyter applications.


- Start of changelog.
