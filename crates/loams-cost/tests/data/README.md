# Bill fixtures (synthetic)

Hand-written for the import tests (CA1 Task 3, design §84 §6.2). They are **not** real bills: the account ids, resources, amounts and tags are made up, and they carry only the columns the importer reads plus a few it ignores. They exist to check parsing, mapping, day splitting and totals.

| File | Format | Lines | Σ cost column |
|---|---|---|---|
| `focus.csv` | FOCUS (1.x core columns) | 6 | EffectiveCost 127.5 |
| `cur2.csv` | AWS CUR 2.0 | 4 | net unblended (unblended where empty) 72.25 |
| `cur-legacy.csv` | legacy CUR | 2 | unblended 30 |
| `generic.csv` | generic CSV (`start=From,end=To,cost=Amount,currency=@EUR,service=Item,tags=Tags`) | 3 | 60 |
