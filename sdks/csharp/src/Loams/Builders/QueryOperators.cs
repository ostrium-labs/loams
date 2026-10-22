// Optional operator sugar on the generated `Query` (SDKP §2.4): `a & b` is
// `Filter.And(a, b)`, `a | b` is `Filter.Or(a, b)`, `~a` is `Filter.Not(a)`.

namespace Loams.Collection.V1;

public sealed partial class Query
{
    public static Query operator &(Query left, Query right) => Loams.Filter.And(left, right);

    public static Query operator |(Query left, Query right) => Loams.Filter.Or(left, right);

    public static Query operator ~(Query query) => Loams.Filter.Not(query);
}
