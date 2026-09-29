@echo off
REM Dev helper: run the Dart analysis server without stdio pipes.
REM
REM `dart analyze` / `flutter analyze` spawn the analysis server with piped
REM stdio, which some locked-down Windows sandboxes refuse (CreateFile
REM failed 5 / ProcessException: Access denied). Redirecting stdout+stderr
REM to files gives the child inheritable handles instead of pipes, which is
REM permitted, and the analyzer runs normally.
REM
REM Usage:  tool\analyze.bat [paths...]        (default: lib test)

SETLOCAL
SET DART_SDK=F:\flutter_sdk\flutter\bin\cache\dart-sdk
SET SNAPSHOT=%DART_SDK%\bin\snapshots\analysis_server_aot.dart.snapshot
SET RUNTIME=%DART_SDK%\bin\dartaotruntime.exe

SET OUT=%~dp0..\.analyze_out.txt
SET ERR=%~dp0..\.analyze_err.txt

SET TARGETS=%*
IF "%TARGETS%"=="" SET TARGETS=lib test

"%RUNTIME%" "%SNAPSHOT%" --client-id=dart-analyze --sdk "%DART_SDK%" --packages "%~dp0..\.dart_tool\package_config.json" --protocol=stdio < NUL > "%OUT%" 2> "%ERR%"

EXIT /B %ERRORLEVEL%
