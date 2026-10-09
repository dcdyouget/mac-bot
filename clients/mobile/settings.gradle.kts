pluginManagement { repositories { google { content { includeGroupByRegex("com\\.android.*"); includeGroupByRegex("androidx.*") } }; mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { google { content { includeGroupByRegex("com\\.android.*"); includeGroupByRegex("androidx.*"); includeGroupByRegex("com\\.google.*") } }; mavenCentral() } }
rootProject.name = "MacBotMobile"
include(":shared", ":androidApp")
