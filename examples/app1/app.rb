require 'sinatra'

set :bind, '0.0.0.0'
set :port, ENV['PORT'] || 3000

get '/' do
  "Hello from App 1 running on #{`hostname`.strip}"
end
