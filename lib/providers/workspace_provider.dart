import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../services/workspace_service.dart';

final workspaceProjectsProvider = FutureProvider<List<WorkspaceProjectFile>>(
  (ref) => WorkspaceService().projects(),
);

void refreshWorkspace(WidgetRef ref) => ref.invalidate(workspaceProjectsProvider);
