package dev.loams;

import dev.loams.gen.loams.collection.v1.FieldSort;
import dev.loams.gen.loams.collection.v1.MissingOrder;
import dev.loams.gen.loams.collection.v1.PrimaryKeySort;
import dev.loams.gen.loams.collection.v1.ScoreSort;
import dev.loams.gen.loams.collection.v1.SortKey;
import dev.loams.gen.loams.collection.v1.SortOrder;

/** Sort keys for a search (SDKP §2.4). */
public final class Sort {

    private Sort() {}

    /** By score, best first: the order is sent unset, the IR's default for a score (as the builder corpus spells it). */
    public static SortKey score() {
        return SortKey.newBuilder().setScore(ScoreSort.newBuilder()).build();
    }

    /** By primary key, ascending (the order unset, the IR's default): the tiebreaker {@code queryAll} needs. */
    public static SortKey pk() {
        return SortKey.newBuilder().setPk(PrimaryKeySort.newBuilder()).build();
    }

    /** By a fast field; {@code missingFirst} null leaves the engine's default. */
    public static SortKey field(String name, boolean descending, Boolean missingFirst) {
        FieldSort.Builder sort = FieldSort.newBuilder()
                .setField(name)
                .setOrder(descending ? SortOrder.SORT_ORDER_DESC : SortOrder.SORT_ORDER_ASC);
        if (missingFirst != null) {
            sort.setMissing(missingFirst ? MissingOrder.MISSING_ORDER_FIRST : MissingOrder.MISSING_ORDER_LAST);
        }
        return SortKey.newBuilder().setField(sort).build();
    }

    /** By a fast field, ascending. */
    public static SortKey field(String name) {
        return field(name, false, null);
    }
}
