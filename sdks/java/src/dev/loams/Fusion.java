package dev.loams;

import com.google.protobuf.Empty;
import dev.loams.gen.loams.collection.v1.Rrf;
import dev.loams.gen.loams.collection.v1.WeightedSum;

/** Fusion methods for a hybrid search (SDKP §2.4). */
public final class Fusion {

    private Fusion() {}

    /** Reciprocal rank fusion with constant {@code k}. */
    public static dev.loams.gen.loams.collection.v1.Fusion rrf(int k) {
        return dev.loams.gen.loams.collection.v1.Fusion.newBuilder().setRrf(Rrf.newBuilder().setK(k)).build();
    }

    /** Reciprocal rank fusion with k = 60. */
    public static dev.loams.gen.loams.collection.v1.Fusion rrf() {
        return rrf(60);
    }

    /** Distribution-based score fusion. */
    public static dev.loams.gen.loams.collection.v1.Fusion dbsf() {
        return dev.loams.gen.loams.collection.v1.Fusion.newBuilder().setDbsf(Empty.getDefaultInstance()).build();
    }

    /** A weighted sum of the retrievers' normalised scores, one weight per retriever. */
    public static dev.loams.gen.loams.collection.v1.Fusion weighted(float... weights) {
        WeightedSum.Builder sum = WeightedSum.newBuilder();
        for (float w : weights) {
            sum.addWeights(w);
        }
        return dev.loams.gen.loams.collection.v1.Fusion.newBuilder().setWeightedSum(sum).build();
    }
}
