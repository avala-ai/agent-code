import 'package:agent_code_client/services/config_service.dart';

Map<String, String> readConfig() => ConfigService().read();
void saveConfig(String key, String value) => ConfigService().set(key, value);
