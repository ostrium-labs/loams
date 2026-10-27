package dev.loams;

import dev.loams.gen.loams.collection.v1.Consistency;
import dev.loams.gen.loams.sql.v1.Column;
import dev.loams.gen.loams.sql.v1.QueryArrowRequest;
import dev.loams.gen.loams.sql.v1.QueryArrowResponse;
import dev.loams.gen.loams.sql.v1.QueryRequest;
import dev.loams.gen.loams.sql.v1.QueryResponse;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * {@code loams.sql} (SDKP §2.5, Part C): one read-only statement over a namespace's collections,
 * with {@code $n} parameters ({@code loams.sql.v1}, SDKP P0b).
 */
public final class SqlModule {
    private final CallInvoker invoker;

    SqlModule(CallInvoker invoker) {
        this.invoker = invoker;
    }

    /** Runs one statement and answers its columns and rows. */
    public QueryResponse query(QueryRequest request, CallOptions... options) {
        return invoker.unary(Modules.binding("sql", "Query"), request, CallOptions.merge(options), QueryResponse.parser());
    }

    /**
     * Runs {@code sql} with {@code params} bound to {@code $1…$n} (a count mismatch is {@code
     * invalid_argument}, reason {@code sql_param_count}).
     */
    public QueryResponse query(String sql, List<?> params, CallOptions... options) {
        return query(sql, params, null, null, options);
    }

    /** Runs a statement with parameters, a row cap and a consistency (either may be null). */
    public QueryResponse query(String sql, List<?> params, Integer maxRows, Consistency consistency, CallOptions... options) {
        QueryRequest.Builder request = QueryRequest.newBuilder().setSql(sql);
        if (params != null) {
            for (Object param : params) {
                request.addParams(Values.of(param));
            }
        }
        if (maxRows != null) {
            request.setMaxRows(maxRows);
        }
        if (consistency != null) {
            request.setConsistency(consistency);
        }
        return query(request.build(), options);
    }

    /**
     * Runs one statement and answers its result as one Arrow IPC stream ({@code arrow_ipc}); this
     * SDK carries no Arrow reader, so the bytes are the caller's to read (Arrow Java's {@code
     * ArrowStreamReader} does).
     */
    public QueryArrowResponse queryArrow(QueryArrowRequest request, CallOptions... options) {
        return invoker.unary(
                Modules.binding("sql", "QueryArrow"), request, CallOptions.merge(options), QueryArrowResponse.parser());
    }

    /** Each row of an answer as a map of column name to plain value. */
    public static List<Map<String, Object>> records(QueryResponse response) {
        List<Map<String, Object>> out = new ArrayList<>();
        for (var row : response.getRowsList()) {
            Map<String, Object> record = new LinkedHashMap<>();
            for (int i = 0; i < response.getColumnsCount(); i++) {
                Column column = response.getColumns(i);
                record.put(column.getName(), i < row.getValuesCount() ? Values.toPlain(row.getValues(i)) : null);
            }
            out.add(record);
        }
        return out;
    }
}
