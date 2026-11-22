# Cost and data transmission

The maintenance worker (Sonnet by default) runs with **your own Claude authentication**. The repository contents the worker reads are sent to Anthropic, and Claude usage charges (consumption) are incurred on every run. The worker runs at most once per push, with `worker.timeout_secs` as its upper bound.
