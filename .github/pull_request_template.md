## Summary

<!-- Describe your changes in 2-3 sentences. What problem does this solve? -->

## Type of Change

<!-- Mark with [x] all that apply -->

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactoring (no functional changes)
- [ ] Performance improvement
- [ ] CI/CD changes

## Changes Made

<!-- List the specific changes made in this PR -->

-

## Testing

<!-- Describe how you tested your changes -->

- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing performed

**Test commands run:**
```bash
cargo test
cargo clippy
```

## Checklist

<!-- Mark with [x] when complete -->

### Code Quality
- [ ] Code follows project style guidelines (`cargo fmt`)
- [ ] No new Clippy warnings (`cargo clippy -- -D warnings`)
- [ ] Public APIs have documentation comments
- [ ] Error handling is appropriate (no unwrap in production code)

### Testing
- [ ] Tests cover the happy path
- [ ] Tests cover error cases
- [ ] All existing tests pass

### Documentation
- [ ] CLAUDE.md updated (if architectural changes)
- [ ] README updated (if user-facing changes)
- [ ] Code comments explain "why" not "what"

### Security
- [ ] No secrets or credentials committed
- [ ] Input validation added where needed
- [ ] No new security vulnerabilities introduced

## Related Issues

<!-- Link related issues using: Fixes #123, Closes #456 -->

## Screenshots (if applicable)

<!-- Add screenshots for UI changes -->

## Additional Notes

<!-- Any additional context, trade-offs, or future improvements -->
