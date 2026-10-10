class PageResult<T> {
  final List<T> items;
  final String? nextPageToken;

  PageResult({required this.items, this.nextPageToken});
}

class Pagination {
  static Stream<T> iterate<T>(Future<PageResult<T>> Function(String pageToken) fetcher) async* {
    String pageToken = '';

    while (true) {
      final page = await fetcher(pageToken);
      for (final item in page.items) {
        yield item;
      }

      final next = page.nextPageToken;
      if (next == null || next.isEmpty || next == pageToken) {
        break;
      }
      pageToken = next;
    }
  }
}
