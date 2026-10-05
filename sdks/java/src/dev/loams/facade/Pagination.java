package dev.loams.facade;

/**
 * The two fields a paged call pages on, as {@code FacadeOptions.pagination}
 * ({@code "<items>:<next page token>"}) names them.
 *
 * <p>The field names are proto field names rather than accessor names, because the iterator
 * reaches them through protobuf reflection rather than through generated getters — that is
 * what lets one iterator serve every paged RPC instead of one per list RPC (R6).
 *
 * @param itemsField the response's repeated field
 * @param nextPageTokenField the response's token field
 * @param pageSizeField the request's {@code page_size} field
 * @param pageTokenField the request's token field
 */
public record Pagination(
        String itemsField,
        String nextPageTokenField,
        String pageSizeField,
        String pageTokenField) {

    /**
     * The request field name with the AIP-158 default filled in, so a binding that names no
     * specific field still pages on {@code page_size}.
     */
    public String pageSizeFieldOrDefault() {
        return pageSizeField == null || pageSizeField.isEmpty() ? "page_size" : pageSizeField;
    }

    /**
     * The request token field with the AIP-158 default filled in.
     */
    public String pageTokenFieldOrDefault() {
        return pageTokenField == null || pageTokenField.isEmpty() ? "page_token" : pageTokenField;
    }
}