# Editor setup

pyprojx includes a language server, started with `pyprojx server`, which checks
`pyproject.toml` as you edit it:

- diagnostics as you type, each saying which tool versions it was checked
  against and where they came from, such as `uv.lock`;
- quick fixes for the problems pyprojx can fix, with safe fixes marked
  preferred;
- a fix-all action, `source.fixAll.pyprojx`, which applies the safe fixes, as
  `pyprojx check --fix` does, and which editors can run on save.

The server needs `pyprojx` on your `PATH`, such as from
`uv tool install pyprojx`. It checks only files named `pyproject.toml`, so it is
safe to start it for all TOML files. It takes tool versions from `uv.lock` or
`pylock.toml` on disk, and reads them again when they change, the next time you
edit `pyproject.toml`.

There is no VS Code extension yet.

## Neovim

With Neovim 0.11 or later, add to your configuration:

```lua
vim.lsp.config('pyprojx', {
  cmd = { 'pyprojx', 'server' },
  filetypes = { 'toml' },
  root_markers = { 'pyproject.toml', '.git' },
})
vim.lsp.enable('pyprojx')
```

Apply a quick fix with `vim.lsp.buf.code_action()` (`gra` by default). To apply
the safe fixes on save:

```lua
vim.api.nvim_create_autocmd('BufWritePre', {
  pattern = 'pyproject.toml',
  callback = function(args)
    local client = vim.lsp.get_clients({ bufnr = args.buf, name = 'pyprojx' })[1]
    if not client then
      return
    end
    local params = vim.lsp.util.make_range_params(0, client.offset_encoding)
    params.context = { only = { 'source.fixAll' }, diagnostics = {} }
    local response = client:request_sync('textDocument/codeAction', params, 1000, args.buf)
    for _, action in ipairs(response and response.result or {}) do
      vim.lsp.util.apply_workspace_edit(action.edit, client.offset_encoding)
    end
  end,
})
```

## Helix

Add to `languages.toml`. Listing `language-servers` replaces Helix's default
servers for TOML, so keep the ones you use, such as `taplo` and `tombi`:

```toml
[language-server.pyprojx]
command = "pyprojx"
args = ["server"]

[[language]]
name = "toml"
language-servers = ["taplo", "tombi", "pyprojx"]
```

## Emacs

With Eglot, which runs one server per buffer, this replaces its default TOML
server, `tombi`:

```elisp
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '((toml-ts-mode conf-toml-mode) . ("pyprojx" "server"))))
```

## Other editors

Configure a language server that runs `pyprojx server` over standard input and
output for TOML files.
