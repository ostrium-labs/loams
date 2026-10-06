import 'dart:io';

abstract class TokenSource {
  Future<String> getToken();
  Future<void> refresh();
}

class StaticTokenSource implements TokenSource {
  final String token;
  StaticTokenSource(this.token);

  @override
  Future<String> getToken() async => token;

  @override
  Future<void> refresh() async {}
}

class EnvTokenSource implements TokenSource {
  final String envVar;
  EnvTokenSource({this.envVar = 'LOAMS_API_KEY'});

  @override
  Future<String> getToken() async {
    return Platform.environment[envVar] ?? '';
  }

  @override
  Future<void> refresh() async {}
}

class RefreshTokenSource implements TokenSource {
  final Future<String> Function() refresher;
  String? _currentToken;

  RefreshTokenSource(this.refresher);

  @override
  Future<String> getToken() async {
    _currentToken ??= await refresher();
    return _currentToken!;
  }

  @override
  Future<void> refresh() async {
    _currentToken = await refresher();
  }
}
