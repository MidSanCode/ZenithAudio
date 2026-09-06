import 'dart:io';
import 'dart:typed_data';

import 'package:path_provider/path_provider.dart';

Future<String> createAudioUrlImpl(Uint8List bytes, String mime) async {
  final dir = await getTemporaryDirectory();
  final path =
      '${dir.path}/preview_${DateTime.now().millisecondsSinceEpoch}.wav';
  final file = File(path);
  await file.writeAsBytes(bytes);
  return Uri.file(path).toString();
}
