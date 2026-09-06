import 'dart:html' as html;
import 'dart:typed_data';

Future<String> createAudioUrlImpl(Uint8List bytes, String mime) async {
  final blob = html.Blob([bytes], mime);
  return html.Url.createObjectUrl(blob);
}
