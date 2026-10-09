# Error Prone annotation metadata references a JDK compiler enum. Android
# instrumentation uses the annotations, not javac annotation processing.
-dontwarn javax.lang.model.element.Modifier
