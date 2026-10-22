# status

A small CLI that reads the engine's feature flags and can gate scripts on them.

```sh
php examples/status/main.php                 # list features
php examples/status/main.php feature.hnsw    # exit 2 if off
```
