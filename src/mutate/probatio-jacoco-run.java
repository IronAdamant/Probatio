import java.io.ByteArrayInputStream;
import java.io.File;
import java.lang.reflect.Array;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.jacoco.agent.rt.IAgent;
import org.jacoco.agent.rt.RT;
import org.jacoco.core.analysis.Analyzer;
import org.jacoco.core.analysis.CoverageBuilder;
import org.jacoco.core.analysis.IClassCoverage;
import org.jacoco.core.analysis.ILine;
import org.jacoco.core.data.ExecutionData;
import org.jacoco.core.data.ExecutionDataReader;
import org.jacoco.core.data.ExecutionDataStore;
import org.junit.runner.JUnitCore;
import org.junit.runner.Request;
import org.junit.runner.Result;

/**
 * One JaCoCo dump per test method.
 * JUnit 4 uses Request.method. JUnit 5 uses one platform launch for the whole list.
 */
public class ProbatioJacocoRun {
  static Object jupiterLauncher;
  static boolean jupiterMissing;
  static String lastNote = "";
  static IAgent agent;
  static StringBuilder text;
  static Map<String, Indexed> classFiles;
  static File packageRoot;
  static Map<String, String> jupiterNames = new HashMap<>();
  static int executed;
  static int recorded;

  static final class Indexed {
    final File file;
    final File root;
    Indexed(File file, File root) {
      this.file = file;
      this.root = root;
    }
  }

  public static void main(String[] args) throws Exception {
    if (args.length != 4) {
      System.err.println("usage: ProbatioJacocoRun classDirs testsFile outFile packageRoot");
      System.exit(2);
    }
    String[] classDirs = args[0].split(File.pathSeparator);
    List<String> tests = Files.readAllLines(new File(args[1]).toPath());
    File out = new File(args[2]);
    packageRoot = new File(args[3]);
    classFiles = indexClasses(classDirs);
    agent = RT.getAgent();
    text = new StringBuilder();
    List<String> jupiter = new ArrayList<>();
    int requested = 0;
    for (String spec : tests) {
      int hash = spec.lastIndexOf('#');
      if (hash <= 0) continue;
      requested++;
      String className = spec.substring(0, hash);
      String method = spec.substring(hash + 1);
      agent.getExecutionData(true);
      if (runJUnit4(className, method)) {
        executed++;
        dump(className + "." + method);
        continue;
      }
      jupiter.add(spec);
    }
    if (!jupiter.isEmpty()) runJupiter(jupiter);
    Files.writeString(out.toPath(), text.toString());
    if (requested > 0 && executed == 0) {
      System.err.println("java coverage: no test executed " + lastNote);
      System.exit(1);
    }
    if (executed > 0 && recorded == 0) {
      System.err.println("java coverage: tests ran but JaCoCo recorded no classes");
      System.exit(1);
    }
    // Integration tests can leave non-daemon threads running (jsoup leaves Netty loops).
    // A normal return would wait on those threads until the parent clock kills the dump.
    System.exit(0);
  }

  static Map<String, Indexed> indexClasses(String[] classDirs) {
    Map<String, Indexed> found = new HashMap<>();
    for (String dir : classDirs) {
      if (dir.isEmpty()) continue;
      File root = new File(dir);
      if (root.isDirectory()) indexDir(root, root, found);
    }
    return found;
  }

  static void indexDir(File root, File dir, Map<String, Indexed> found) {
    File[] kids = dir.listFiles();
    if (kids == null) return;
    for (File kid : kids) {
      if (kid.isDirectory()) {
        indexDir(root, kid, found);
        continue;
      }
      String name = kid.getName();
      if (!name.endsWith(".class")) continue;
      String rel = root.toPath().relativize(kid.toPath()).toString().replace('\\', '/');
      found.put(rel.substring(0, rel.length() - ".class".length()), new Indexed(kid, root));
    }
  }

  static void dump(String testName) throws Exception {
    byte[] data = agent.getExecutionData(false);
    ExecutionDataStore store = new ExecutionDataStore();
    ExecutionDataReader reader = new ExecutionDataReader(new ByteArrayInputStream(data));
    reader.setExecutionDataVisitor(store);
    reader.setSessionInfoVisitor(info -> { });
    reader.read();
    if (!store.getContents().isEmpty()) recorded++;
    text.append("TEST ").append(testName).append('\n');
    CoverageBuilder builder = new CoverageBuilder();
    Analyzer analyzer = new Analyzer(store, builder);
    for (ExecutionData exec : store.getContents()) {
      if (!exec.hasHits()) continue;
      Indexed indexed = classFiles.get(exec.getName());
      if (indexed == null) continue;
      analyzer.analyzeAll(indexed.file);
    }
    for (IClassCoverage cc : builder.getClasses()) {
      String source = cc.getSourceFileName();
      if (source == null) continue;
      Indexed indexed = classFiles.get(cc.getName());
      File file = sourceFile(indexed == null ? null : indexed.root, cc.getPackageName(), source);
      if (file == null || !file.isFile()) continue;
      String rel = packageRoot.toPath().relativize(file.toPath()).toString().replace('\\', '/');
      int first = cc.getFirstLine();
      int last = cc.getLastLine();
      for (int line = first; line <= last; line++) {
        ILine cov = cc.getLine(line);
        if (cov.getInstructionCounter().getCoveredCount() <= 0) continue;
        text.append("HIT ").append(rel).append(' ').append(line).append(' ').append(testName).append('\n');
      }
    }
  }

  /** JUnit 4 only. A Jupiter method has no org.junit.Test, and Request.method must not count as a run. */
  static boolean runJUnit4(String className, String method) {
    // Read the class file. Class.forName would initialize every Jupiter test before the batch.
    if (!classFileMentionsJUnit4(className)) return false;
    try {
      Class<?> type = Class.forName(className);
      if (!hasJUnit4Test(type, method)) return false;
      Result result = new JUnitCore().run(Request.method(type, method));
      if (result.getRunCount() <= 0) return false;
      for (org.junit.runner.notification.Failure failure : result.getFailures()) {
        String message = String.valueOf(failure.getMessage());
        if (message.contains("No tests found") || message.contains("No runnable methods") || message.contains("initializationError")) {
          return false;
        }
      }
      return true;
    } catch (Throwable ignored) {
      return false;
    }
  }

  static boolean classFileMentionsJUnit4(String className) {
    String resource = className.replace('.', '/') + ".class";
    try (java.io.InputStream in = ClassLoader.getSystemResourceAsStream(resource)) {
      if (in == null) return false;
      byte[] bytes = in.readAllBytes();
      String latin = new String(bytes, java.nio.charset.StandardCharsets.ISO_8859_1);
      return latin.contains("Lorg/junit/Test;");
    } catch (java.io.IOException error) {
      return false;
    }
  }

  static boolean hasJUnit4Test(Class<?> type, String method) {
    String simple = jupiterMethod(method);
    for (java.lang.reflect.Method candidate : type.getDeclaredMethods()) {
      if (!candidate.getName().equals(simple)) continue;
      for (java.lang.annotation.Annotation ann : candidate.getDeclaredAnnotations()) {
        if (ann.annotationType().getName().equals("org.junit.Test")) return true;
      }
    }
    return false;
  }

  /** Simple method name. A unique id may append "()" or "(java.lang.String)". */
  static String jupiterMethod(String method) {
    int end = 0;
    while (end < method.length()) {
      char c = method.charAt(end);
      if (Character.isLetterOrDigit(c) || c == '_' || c == '$') end++;
      else break;
    }
    return end == 0 ? method : method.substring(0, end);
  }

  /**
   * Surefire names a parameterized invocation "method(String)[1]".
   * selectMethod(class, name) leaves the parameter list empty, and JUnit 5.14
   * then runs nothing if any one selector fails. Keep the parameter types, and
   * leave out a name that does not resolve.
   */
  static final class JupiterSpec {
    final String className;
    final String simple;
    final Class<?>[] types;
    JupiterSpec(String className, String simple, Class<?>[] types) {
      this.className = className;
      this.simple = simple;
      this.types = types;
    }
  }

  static final Map<String, Class<?>> loadedTests = new HashMap<>();

  static void runJupiter(List<String> specs) {
    if (jupiterMissing) {
      lastNote = "jupiter launcher missing";
      return;
    }
    try {
      Map<String, JupiterSpec> unique = new LinkedHashMap<>();
      int skipped = 0;
      for (String spec : specs) {
        JupiterSpec parsed = parseJupiterSpec(spec);
        if (parsed == null) {
          skipped++;
          continue;
        }
        String key = parsed.className + "#" + parsed.simple + "#" + typeKey(parsed.types);
        unique.putIfAbsent(key, parsed);
        jupiterNames.put(parsed.className + "#" + parsed.simple, parsed.className + "." + parsed.simple);
      }
      if (unique.isEmpty()) {
        lastNote = "jupiter no resolvable methods skipped=" + skipped;
        return;
      }
      if (jupiterLauncher == null) {
        Class<?> factory = Class.forName("org.junit.platform.launcher.core.LauncherFactory");
        jupiterLauncher = factory.getMethod("create").invoke(null);
      }
      Class<?> selectorType = Class.forName("org.junit.platform.engine.DiscoverySelector");
      Class<?> selectorsType = Class.forName("org.junit.platform.engine.discovery.DiscoverySelectors");
      java.lang.reflect.Method select = selectMethodWithTypes(selectorsType);
      boolean typed = select.getParameterCount() == 3;
      Object selectors = Array.newInstance(selectorType, unique.size());
      int index = 0;
      for (JupiterSpec spec : unique.values()) {
        Class<?> type = loadTestClass(spec.className);
        // Platform 1.7 has selectMethod(Class, String) only. 5.14's two-argument
        // form drops the parameter list and can run nothing, so keep the types
        // when that overload exists.
        Object selector = typed
          ? select.invoke(null, new Object[] { type, spec.simple, spec.types })
          : select.invoke(null, new Object[] { type, spec.simple });
        Array.set(selectors, index++, selector);
      }
      Class<?> builderType = Class.forName("org.junit.platform.launcher.core.LauncherDiscoveryRequestBuilder");
      Object builder = builderType.getMethod("request").invoke(null);
      builderType.getMethod("configurationParameter", String.class, String.class)
        .invoke(builder, "junit.jupiter.execution.parallel.enabled", "false");
      builderType.getMethod("selectors", Array.newInstance(selectorType, 0).getClass()).invoke(builder, selectors);
      Object request = builderType.getMethod("build").invoke(builder);
      Class<?> listenerType = Class.forName("org.junit.platform.launcher.TestExecutionListener");
      Object listener = Proxy.newProxyInstance(listenerType.getClassLoader(), new Class<?>[] { listenerType }, new JupiterDump());
      Object listeners = Array.newInstance(listenerType, 1);
      Array.set(listeners, 0, listener);
      Class<?> requestType = Class.forName("org.junit.platform.launcher.LauncherDiscoveryRequest");
      Class<?> launcherType = Class.forName("org.junit.platform.launcher.Launcher");
      launcherType.getMethod("execute", requestType, listenerType.arrayType()).invoke(jupiterLauncher, request, listeners);
      if (executed == 0) lastNote = "jupiter started=0 resolved=" + unique.size() + " skipped=" + skipped;
    } catch (ClassNotFoundException missing) {
      String missingName = String.valueOf(missing.getMessage());
      if (missingName.contains("org.junit.platform")) jupiterMissing = true;
      lastNote = "jupiter missing " + missingName;
    } catch (Throwable error) {
      Throwable cause = error.getCause() == null ? error : error.getCause();
      lastNote = "jupiter " + cause.getClass().getSimpleName() + " " + cause.getMessage();
    }
  }

  static JupiterSpec parseJupiterSpec(String spec) {
    int hash = spec.lastIndexOf('#');
    if (hash <= 0) return null;
    String className = spec.substring(0, hash).trim();
    String method = spec.substring(hash + 1).trim();
    while (method.endsWith("]")) {
      int open = method.lastIndexOf('[');
      if (open <= 0) return null;
      method = method.substring(0, open).trim();
    }
    String simple;
    String params;
    int paren = method.indexOf('(');
    if (paren < 0) {
      simple = method;
      params = null;
    } else if (method.endsWith(")")) {
      simple = method.substring(0, paren).trim();
      params = method.substring(paren + 1, method.length() - 1).trim();
    } else {
      return null;
    }
    if (!isJavaIdentifier(simple) || className.isEmpty()) return null;
    Class<?>[] types = parameterTypes(className, simple, params);
    if (types == null) return null;
    return new JupiterSpec(className, simple, types);
  }

  static boolean isJavaIdentifier(String name) {
    if (name.isEmpty()) return false;
    if (!Character.isJavaIdentifierStart(name.charAt(0))) return false;
    for (int i = 1; i < name.length(); i++) {
      if (!Character.isJavaIdentifierPart(name.charAt(i))) return false;
    }
    return true;
  }

  /** params is null when the report gave no signature. An empty string is "()". */
  static Class<?>[] parameterTypes(String className, String simple, String params) {
    Class<?> type = loadTestClass(className);
    if (type == null) return null;
    List<java.lang.reflect.Method> named = new ArrayList<>();
    for (Class<?> cursor = type; cursor != null && cursor != Object.class; cursor = cursor.getSuperclass()) {
      java.lang.reflect.Method[] methods;
      try {
        methods = cursor.getDeclaredMethods();
      } catch (Throwable error) {
        continue;
      }
      for (java.lang.reflect.Method candidate : methods) {
        if (candidate.isBridge() || candidate.isSynthetic()) continue;
        if (candidate.getName().equals(simple)) named.add(candidate);
      }
    }
    if (named.isEmpty()) return null;
    if (params == null) {
      java.lang.reflect.Method noArg = null;
      for (java.lang.reflect.Method candidate : named) {
        if (candidate.getParameterCount() == 0) noArg = candidate;
      }
      if (noArg != null) return noArg.getParameterTypes();
      return named.size() == 1 ? named.get(0).getParameterTypes() : null;
    }
    String[] wanted = splitParams(params);
    java.lang.reflect.Method match = null;
    for (java.lang.reflect.Method candidate : named) {
      if (!paramsMatch(candidate.getParameterTypes(), wanted)) continue;
      if (match != null) return null;
      match = candidate;
    }
    return match == null ? null : match.getParameterTypes();
  }

  static Class<?> loadTestClass(String className) {
    if (loadedTests.containsKey(className)) return loadedTests.get(className);
    try {
      Class<?> type = Class.forName(className);
      loadedTests.put(className, type);
      return type;
    } catch (Throwable error) {
      loadedTests.put(className, null);
      return null;
    }
  }

  static String[] splitParams(String params) {
    if (params.isEmpty()) return new String[0];
    List<String> parts = new ArrayList<>();
    int depth = 0;
    int start = 0;
    for (int i = 0; i < params.length(); i++) {
      char c = params.charAt(i);
      if (c == '<' || c == '(') depth++;
      else if (c == '>' || c == ')') depth = Math.max(0, depth - 1);
      else if (c == ',' && depth == 0) {
        parts.add(params.substring(start, i).trim());
        start = i + 1;
      }
    }
    String tail = params.substring(start).trim();
    if (!tail.isEmpty()) parts.add(tail);
    return parts.toArray(new String[0]);
  }

  static boolean paramsMatch(Class<?>[] actual, String[] wanted) {
    if (actual.length != wanted.length) return false;
    for (int i = 0; i < actual.length; i++) {
      if (!typeMatches(actual[i], wanted[i])) return false;
    }
    return true;
  }

  static boolean typeMatches(Class<?> actual, String wanted) {
    String name = wanted.trim();
    if (name.endsWith("...")) name = name.substring(0, name.length() - 3).trim() + "[]";
    int dims = 0;
    while (name.endsWith("[]")) {
      dims++;
      name = name.substring(0, name.length() - 2).trim();
    }
    Class<?> cursor = actual;
    for (int i = 0; i < dims; i++) {
      if (cursor == null || !cursor.isArray()) return false;
      cursor = cursor.getComponentType();
    }
    if (cursor == null || cursor.isArray() || name.isEmpty()) return false;
    if (name.equals(cursor.getSimpleName()) || name.equals(cursor.getName())) return true;
    String binary = cursor.getName().replace('$', '.');
    if (name.equals(binary) || binary.endsWith("." + name)) return true;
    String canon = cursor.getCanonicalName();
    return canon != null && (name.equals(canon) || canon.endsWith("." + name));
  }

  static String typeKey(Class<?>[] types) {
    StringBuilder key = new StringBuilder();
    for (Class<?> type : types) {
      if (key.length() > 0) key.append(',');
      key.append(type.getName());
    }
    return key.toString();
  }

  static java.lang.reflect.Method selectMethodWithTypes(Class<?> selectorsType) throws NoSuchMethodException {
    java.lang.reflect.Method byName = null;
    for (java.lang.reflect.Method candidate : selectorsType.getMethods()) {
      if (!candidate.getName().equals("selectMethod")) continue;
      Class<?>[] params = candidate.getParameterTypes();
      if (params.length == 3 && params[0] == Class.class && params[1] == String.class && params[2].isArray() && params[2].getComponentType() == Class.class) {
        return candidate;
      }
      if (params.length == 2 && params[0] == Class.class && params[1] == String.class) byName = candidate;
    }
    if (byName != null) return byName;
    throw new NoSuchMethodException("selectMethod(Class, String, Class[])");
  }

  static final class JupiterDump implements InvocationHandler {
    public Object invoke(Object proxy, java.lang.reflect.Method method, Object[] args) {
      if (method.getDeclaringClass() == Object.class) {
        if (method.getName().equals("toString")) return "ProbatioJupiterDump";
        if (method.getName().equals("hashCode")) return System.identityHashCode(proxy);
        if (method.getName().equals("equals")) return args != null && args.length > 0 && proxy == args[0];
      }
      if (args == null || args.length == 0) return null;
      try {
        if (method.getName().equals("executionStarted")) noteStart(args[0]);
        else if (method.getName().equals("executionFinished")) noteFinish(args[0]);
      } catch (Throwable error) {
        lastNote = "jupiter dump " + error.getClass().getSimpleName();
      }
      return null;
    }
  }

  static String activeTest;

  static void noteStart(Object identifier) throws Exception {
    String name = testName(identifier);
    if (name == null) return;
    activeTest = name;
    agent.getExecutionData(true);
  }

  static void noteFinish(Object identifier) throws Exception {
    String name = testName(identifier);
    if (name == null || !name.equals(activeTest)) return;
    dump(name);
    executed++;
    activeTest = null;
  }

  static String testName(Object identifier) throws Exception {
    if (!Boolean.TRUE.equals(identifier.getClass().getMethod("isTest").invoke(identifier))) return null;
    Object uid = identifier.getClass().getMethod("getUniqueId").invoke(identifier);
    String text = String.valueOf(uid);
    String className = lastSegment(text, "nested-class");
    if (className == null) className = lastSegment(text, "class");
    String method = lastSegment(text, "method");
    if (method == null) method = lastSegment(text, "test-template");
    if (className == null || method == null) return null;
    if (className.indexOf('.') < 0) {
      String outer = lastSegment(text, "class");
      if (outer != null && outer.indexOf('.') >= 0) className = outer + "$" + className;
    }
    return jupiterNames.get(className + "#" + jupiterMethod(method));
  }

  static String lastSegment(String uid, String kind) {
    String token = "[" + kind + ":";
    int start = uid.lastIndexOf(token);
    if (start < 0) return null;
    int end = uid.indexOf(']', start);
    if (end < 0) return null;
    return uid.substring(start + token.length(), end);
  }

  static File sourceFile(File classes, String pkg, String source) {
    if (classes == null) return null;
    File target = classes.getParentFile();
    File module = target == null ? null : target.getParentFile();
    if (module == null) return null;
    String rel = (pkg == null || pkg.isEmpty() ? "" : pkg + "/") + source;
    return new File(module, "src/main/java/" + rel);
  }
}
