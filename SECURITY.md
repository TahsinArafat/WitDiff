# Security policy

WitDiff executes repository-configured test commands and therefore must be treated like a build/test tool: running it on an untrusted repository can execute untrusted code.

v0.1 does not provide a sandbox.

Do not run WitDiff against untrusted code with production credentials, cloud metadata access, SSH agent access, or sensitive environment variables. Sandboxed execution is on the roadmap.

Please report vulnerabilities privately to the maintainers rather than opening a public exploit issue.
