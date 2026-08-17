# Example fixture — not a template to reuse

Worked example of a fixture, kept to show the shape. It tests one specific
question: does a rule forbidding `pip` in favour of `uv` help or hurt?

**Every incident needs its own fixture.** Generate one with
`scripts/new-fixture.py <topic-slug>`; do not point the arena at this directory
and call the result a proof of something else.

What makes this example worth reading: `verify.py` checks that
`requires-python` was not silently lowered. That is the workaround a real run
actually took when it could not satisfy the rule — proof that checking only the
happy path would have passed a cheat.

# salesreport

Python 3.14 project. Dependencies are declared in `pyproject.toml`.
