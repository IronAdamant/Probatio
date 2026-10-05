import java.io.ByteArrayInputStream;
import java.io.File;
import java.nio.file.Files;
import java.util.List;
import org.jacoco.agent.rt.IAgent;
import org.jacoco.agent.rt.RT;
import org.jacoco.core.analysis.Analyzer;
import org.jacoco.core.analysis.CoverageBuilder;
import org.jacoco.core.analysis.IClassCoverage;
import org.jacoco.core.analysis.ILine;
import org.jacoco.core.data.ExecutionDataReader;
import org.jacoco.core.data.ExecutionDataStore;
import org.junit.runner.JUnitCore;
import org.junit.runner.Request;

/** One JaCoCo dump per JUnit 4 method. Prints "HIT <file> <line>" lines. */
public class ProbatioJacocoRun {
  public static void main(String[] args) throws Exception {
    if (args.length != 4) {
      System.err.println("usage: ProbatioJacocoRun classDirs testsFile outFile packageRoot");
      System.exit(2);
    }
    String[] classDirs = args[0].split(File.pathSeparator);
    List<String> tests = Files.readAllLines(new File(args[1]).toPath());
    File out = new File(args[2]);
    File root = new File(args[3]);
    IAgent agent = RT.getAgent();
    StringBuilder text = new StringBuilder();
    for (String spec : tests) {
      int hash = spec.lastIndexOf('#');
      if (hash <= 0) continue;
      String className = spec.substring(0, hash);
      String method = spec.substring(hash + 1);
      agent.getExecutionData(true);
      try {
        new JUnitCore().run(Request.method(Class.forName(className), method));
      } catch (Throwable ignored) {
        continue;
      }
      byte[] data = agent.getExecutionData(false);
      ExecutionDataStore store = new ExecutionDataStore();
      ExecutionDataReader reader = new ExecutionDataReader(new ByteArrayInputStream(data));
      reader.setExecutionDataVisitor(store);
      reader.setSessionInfoVisitor(info -> { });
      reader.read();
      String testName = className + "." + method;
      text.append("TEST ").append(testName).append('\n');
      for (String dir : classDirs) {
        if (dir.isEmpty()) continue;
        File classes = new File(dir);
        if (!classes.isDirectory()) continue;
        CoverageBuilder builder = new CoverageBuilder();
        Analyzer analyzer = new Analyzer(store, builder);
        analyzer.analyzeAll(classes);
        for (IClassCoverage cc : builder.getClasses()) {
          String source = cc.getSourceFileName();
          if (source == null) continue;
          File file = sourceFile(classes, cc.getPackageName(), source);
          if (file == null || !file.isFile()) continue;
          String rel = root.toPath().relativize(file.toPath()).toString().replace('\\', '/');
          int first = cc.getFirstLine();
          int last = cc.getLastLine();
          for (int line = first; line <= last; line++) {
            ILine cov = cc.getLine(line);
            if (cov.getInstructionCounter().getCoveredCount() <= 0) continue;
            text.append("HIT ").append(rel).append(' ').append(line).append(' ').append(testName).append('\n');
          }
        }
      }
    }
    Files.writeString(out.toPath(), text.toString());
  }

  static File sourceFile(File classes, String pkg, String source) {
    File target = classes.getParentFile();
    File module = target == null ? null : target.getParentFile();
    if (module == null) return null;
    String rel = (pkg == null || pkg.isEmpty() ? "" : pkg + "/") + source;
    return new File(module, "src/main/java/" + rel);
  }
}
