import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:uuid/uuid.dart';

final floatingWindowProvider =
    NotifierProvider<FloatingWindowNotifier, List<FloatingWindowInstance>>(
  FloatingWindowNotifier.new,
);

class FloatingWindowInstance {
  final String id;
  final String title;
  final WidgetBuilder builder;
  final Future<void> Function()? onBeforeClose;
  Offset position;
  Size size;
  bool isMinimized;

  FloatingWindowInstance({
    required this.id,
    required this.title,
    required this.builder,
    this.onBeforeClose,
    this.position = const Offset(120, 80),
    this.size = const Size(560, 420),
    this.isMinimized = false,
  });
}

class FloatingWindowNotifier extends Notifier<List<FloatingWindowInstance>> {
  static const _uuid = Uuid();
  int _cascadeIndex = 0;

  @override
  List<FloatingWindowInstance> build() => [];

  String open({
    required String title,
    required WidgetBuilder builder,
    Future<void> Function()? onBeforeClose,
    Size? size,
  }) {
    final id = _uuid.v4();
    final cascadeOffset = Offset(
      120 + (_cascadeIndex % 8) * 30,
      80 + (_cascadeIndex % 8) * 30,
    );
    _cascadeIndex++;
    state = [
      ...state,
      FloatingWindowInstance(
        id: id,
        title: title,
        builder: builder,
        onBeforeClose: onBeforeClose,
        position: cascadeOffset,
        size: size ?? const Size(560, 420),
      ),
    ];
    return id;
  }

  Future<void> close(String id) async {
    final window = state.where((w) => w.id == id).firstOrNull;
    if (window?.onBeforeClose != null) {
      await window!.onBeforeClose!();
    }
    state = state.where((w) => w.id != id).toList();
  }

  void toggleMinimize(String id) {
    state = state.map((w) {
      if (w.id == id) {
        w.isMinimized = !w.isMinimized;
      }
      return w;
    }).toList();
  }

  void updatePosition(String id, Offset position) {
    state = state.map((w) {
      if (w.id == id) {
        w.position = position;
      }
      return w;
    }).toList();
  }

  void updateSize(String id, Size size) {
    state = state.map((w) {
      if (w.id == id) {
        w.size = size;
      }
      return w;
    }).toList();
  }

  void bringToFront(String id) {
    final idx = state.indexWhere((w) => w.id == id);
    if (idx < 0 || idx == state.length - 1) return;
    final window = state[idx];
    state = [...state.where((w) => w.id != id), window];
  }
}
