package dev.loams;

import dev.loams.gen.loams.collection.v1.DocumentId;
import dev.loams.gen.loams.collection.v1.WriteDocument;
import dev.loams.gen.loams.collection.v1.WriteOp;
import java.util.Map;

/**
 * One document to upsert: an id, its source and its vectors.
 *
 * @param id a long, a String, a UUID or a {@link DocumentId}
 * @param source a {@code Struct}, a Map with string keys, or a record
 * @param vectors dense vectors by name, or null
 */
public record Doc(Object id, Object source, Map<String, float[]> vectors) {

    /** A document without vectors. */
    public Doc(Object id, Object source) {
        this(id, source, null);
    }

    /** The write op that upserts this document. */
    public WriteOp toOp() {
        WriteDocument.Builder doc = WriteDocument.newBuilder()
                .setId(Values.toId(id))
                .setSource(Values.structFrom(source));
        if (vectors != null) {
            vectors.forEach((name, vector) -> doc.putVectors(name, Values.of(vector)));
        }
        return WriteOp.newBuilder().setUpsert(doc).build();
    }
}
